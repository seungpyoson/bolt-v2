//! Gate 1 — accepted-data ledger and source-proof acceptance.
//!
//! Implements the `backfill-source-proof.v1` contract
//! (`specs/023-nt-research-analytics-platform/reference/backfill-source-proof-schema.md`)
//! as typed Rust. A source family may become canonical NautilusTrader catalog
//! input or backtest input only after an accepted [`SourceProofReport`] exists
//! for the binding, fixture, fidelity class, and claim limits, and every
//! required check has passed.
//!
//! This module owns two responsibilities:
//!
//! 1. Typed source-proof records and the acceptance decision that turns a
//!    candidate proof into an accepted, immutable record.
//! 2. The accepted-data ledger: given an accepted proof plus an ingest-manifest
//!    payload record plus the verified content hash of a staged object, decide
//!    whether that exact object is admissible as backtest input. Anything
//!    missing a source proof, manifest record, content hash, schema sample, or
//!    coverage is rejected.
//!
//! No backtest may consume raw staged data directly. The only path to backtest
//! input is through an [`AcceptedDataset`] produced here.

use std::{
    io::Read,
    path::{Path, PathBuf},
};

use chrono::{DateTime, NaiveDate};
use serde::{Deserialize, Serialize};

/// Governing backfill table contract version for this slice.
pub const CONTRACT_VERSION: &str = "backfill-table-contract.v1";

/// Source-proof schema version implemented by this module.
pub const SOURCE_PROOF_SCHEMA_VERSION: &str = "backfill-source-proof.v1";

const SOURCE_BINDINGS_REGISTRY: &str = include_str!(
    "../../../specs/023-nt-research-analytics-platform/reference/backfill-source-bindings.v1.toml"
);

#[derive(Debug, Deserialize)]
pub struct SourceBindingRegistry {
    #[serde(rename = "source_binding", default)]
    source_bindings: Vec<SourceBindingConfig>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceBindingMetadata {
    pub key: String,
    pub venue: String,
    pub product_family: String,
    pub market_structure_fixture: Option<FixtureType>,
    pub evidence_state: EvidenceState,
    pub table_families: Vec<String>,
    pub required_cross_market_component_roles: Vec<String>,
}

impl SourceBindingRegistry {
    pub fn from_toml_str(text: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(text)
    }

    fn source_binding_config(
        &self,
        source_binding: &str,
        venue: &str,
    ) -> Option<SourceBindingConfig> {
        if source_binding.trim().is_empty() || venue.trim().is_empty() {
            return None;
        }
        self.source_bindings
            .iter()
            .find(|binding| binding.key == source_binding && binding.venue == venue)
            .cloned()
    }

    pub fn source_binding_metadata(
        &self,
        source_binding: &str,
        venue: &str,
    ) -> Option<SourceBindingMetadata> {
        self.source_binding_config(source_binding, venue)
            .map(SourceBindingConfig::into_metadata)
    }

    /// Every configured binding as metadata, in registry order. Gates that
    /// evaluate the whole registry consume this instead of re-parsing the
    /// TOML themselves, so one typed parse owns the binding schema.
    #[must_use]
    pub fn all_binding_metadata(&self) -> Vec<SourceBindingMetadata> {
        self.source_bindings
            .iter()
            .cloned()
            .map(SourceBindingConfig::into_metadata)
            .collect()
    }
}

pub fn committed_source_binding_registry() -> SourceBindingRegistry {
    SourceBindingRegistry::from_toml_str(SOURCE_BINDINGS_REGISTRY)
        .expect("committed source binding registry parses")
}

pub fn resolve_source_bindings_path(path: &Path) -> PathBuf {
    if path.exists() || path.is_absolute() {
        return path.to_path_buf();
    }
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let repo_relative = repo_root.join(path);
    if repo_relative.exists() {
        repo_relative
    } else {
        path.to_path_buf()
    }
}

/// Read a source-binding registry from a (possibly repo-relative) path.
/// Single source of truth for loading a configured registry from disk.
pub fn read_source_binding_registry_from_path(
    path: &std::path::Path,
) -> std::io::Result<SourceBindingRegistry> {
    let resolved = resolve_source_bindings_path(path);
    let mut registry_file =
        crate::io_safety::open_regular_file(&resolved, "source-binding registry").map_err(
            |error| {
                let kind = error
                    .downcast_ref::<std::io::Error>()
                    .map_or(std::io::ErrorKind::InvalidInput, std::io::Error::kind);
                std::io::Error::new(kind, format!("{error:#}"))
            },
        )?;
    let mut text = String::new();
    registry_file.read_to_string(&mut text)?;
    SourceBindingRegistry::from_toml_str(&text)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
}

impl SourceBindingConfig {
    fn into_metadata(self) -> SourceBindingMetadata {
        SourceBindingMetadata {
            key: self.key,
            venue: self.venue,
            product_family: self.product_family,
            market_structure_fixture: self.market_structure_fixture,
            evidence_state: self.evidence_state,
            table_families: self.table_families,
            required_cross_market_component_roles: self.required_cross_market_component_roles,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
struct SourceBindingConfig {
    key: String,
    venue: String,
    product_family: String,
    #[serde(default)]
    market_structure_fixture: Option<FixtureType>,
    source_uri: String,
    evidence_state: EvidenceState,
    #[serde(default)]
    table_families: Vec<String>,
    #[serde(default)]
    required_cross_market_component_roles: Vec<String>,
}

/// Lifecycle status of a source-proof record.
///
/// Accepted records are immutable; a changed fact creates a new
/// [`SourceProofReport::source_proof_version`] (or a new id) that supersedes the
/// prior accepted record rather than mutating it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceProofStatus {
    Pending,
    Accepted,
    Rejected,
}

/// Evidence state assigned in the backfill table contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceState {
    DirectlyBackfillable,
    OwnerArchiveBackfillable,
    BoundedOrCurrentOnly,
    PendingSourceProof,
    VendorOrForwardCaptureOnly,
    NotApplicable,
    ExcludedFromCurrentScope,
}

/// Candidate class used by source selection before any provider is promoted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceCandidateClass {
    OfficialFree,
    PaidVendor,
    ForwardCapture,
}

/// Selection outcome for a source-proof candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SourceSelectionStatus {
    AcceptedForRequiredFidelity,
    AcceptedLowerFidelity,
    Rejected,
    PendingMoreProof,
    ForwardCapturePending,
}

/// Source-proof use boundary before a candidate can become canonical input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceProofUsageScope {
    CanonicalBackfillInput,
    OneOffBackfillData,
}

fn default_source_proof_usage_scope() -> SourceProofUsageScope {
    SourceProofUsageScope::CanonicalBackfillInput
}

/// Market-structure fixture family the proof belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FixtureType {
    BinaryOption,
    PredictionMarket,
    PerpsSpot,
    Options,
    Mixed,
}

/// Data fidelity class in the source-proof vocabulary.
///
/// This is the source-proof fidelity vocabulary from
/// `backfill-source-proof.v1`. Native trade prints map to [`Self::TradeReplay`],
/// which caps results to trade/price-path replay and forbids execution-quality,
/// queue-position, and order-book-liquidity claims. [`Self::QuoteReplay`],
/// [`Self::IndexReplay`], [`Self::MarkReplay`], and [`Self::FundingReplay`] are
/// the point/quote replay classes for NT quote, index-price, mark-price, and
/// funding-rate streams respectively.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SourceProofFidelityClass {
    L2Replay,
    SnapshotReplay,
    TradeReplay,
    TradeBarReplay,
    QuoteReplay,
    IndexReplay,
    MarkReplay,
    FundingReplay,
    MetadataOnly,
    SignalOnly,
    ForwardCapturePending,
}

/// NautilusTrader catalog-mapping status for the normalized rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NtMappingStatus {
    Accepted,
    Pending,
    Rejected,
    NotApplicable,
}

/// Outcome of a single required check.
///
/// [`Self::Passed`] contributes directly to acceptance. [`Self::NotApplicable`]
/// contributes only when a structured claim-limit record binds the same
/// evidence reference. Any `Failed` or `Pending` check keeps the proof out of
/// canonical backfill/catalog/backtest selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckOutcome {
    Passed,
    Failed,
    Pending,
    NotApplicable,
}

impl CheckOutcome {
    #[must_use]
    pub const fn is_acceptable(self) -> bool {
        matches!(self, Self::Passed | Self::NotApplicable)
    }

    #[must_use]
    pub const fn is_not_applicable(self) -> bool {
        matches!(self, Self::NotApplicable)
    }
}

/// How acceptance was reached for an accepted proof.
///
/// `Automated` means every required check was machine-verifiable and passed.
/// `Manual` means at least one check required a recorded human attestation
/// (for example a license review) before all checks could pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AcceptanceMode {
    Automated,
    Manual,
}

/// Machine-readable license/use boundary for a source proof.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum LicenseScope {
    Personal,
    Commercial,
    Enterprise,
    Public,
    #[default]
    Unknown,
    Waived,
}

/// A single required check result with its supporting evidence pointer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequiredCheck {
    pub outcome: CheckOutcome,
    /// Pointer to the evidence backing this check (URI, hash, manifest id, or
    /// recorded attestation). Required for an accepted proof.
    pub evidence_ref: String,
    /// Optional UTC expiry for the evidence backing this check. If present, it
    /// must remain valid through the proof coverage end.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at_utc: Option<String>,
}

impl RequiredCheck {
    #[must_use]
    pub fn passed(evidence_ref: impl Into<String>) -> Self {
        Self {
            outcome: CheckOutcome::Passed,
            evidence_ref: evidence_ref.into(),
            expires_at_utc: None,
        }
    }

    #[must_use]
    pub fn pending(evidence_ref: impl Into<String>) -> Self {
        Self {
            outcome: CheckOutcome::Pending,
            evidence_ref: evidence_ref.into(),
            expires_at_utc: None,
        }
    }

    fn is_acceptable(&self) -> bool {
        self.outcome.is_acceptable() && !self.evidence_ref.trim().is_empty()
    }
}

/// The full set of required checks from `backfill-source-proof.v1`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequiredChecks {
    pub source_access: RequiredCheck,
    pub license: RequiredCheck,
    pub schema: RequiredCheck,
    pub time_semantics: RequiredCheck,
    pub instrument_universe: RequiredCheck,
    pub coverage: RequiredCheck,
    pub retention_freshness: RequiredCheck,
    pub granularity: RequiredCheck,
    pub completeness: RequiredCheck,
    pub nt_mapping: RequiredCheck,
    pub cost: RequiredCheck,
    pub storage: RequiredCheck,
}

impl RequiredChecks {
    const NAMES: [&'static str; 12] = [
        "source_access",
        "license",
        "schema",
        "time_semantics",
        "instrument_universe",
        "coverage",
        "retention_freshness",
        "granularity",
        "completeness",
        "nt_mapping",
        "cost",
        "storage",
    ];

    fn as_slice(&self) -> [&RequiredCheck; 12] {
        [
            &self.source_access,
            &self.license,
            &self.schema,
            &self.time_semantics,
            &self.instrument_universe,
            &self.coverage,
            &self.retention_freshness,
            &self.granularity,
            &self.completeness,
            &self.nt_mapping,
            &self.cost,
            &self.storage,
        ]
    }

    #[cfg(test)]
    fn as_mut_slice(&mut self) -> [&mut RequiredCheck; 12] {
        [
            &mut self.source_access,
            &mut self.license,
            &mut self.schema,
            &mut self.time_semantics,
            &mut self.instrument_universe,
            &mut self.coverage,
            &mut self.retention_freshness,
            &mut self.granularity,
            &mut self.completeness,
            &mut self.nt_mapping,
            &mut self.cost,
            &mut self.storage,
        ]
    }

    /// Names of checks that are not acceptable (failed, pending, or missing
    /// evidence), in declaration order. Empty when every check is acceptable.
    #[must_use]
    pub fn unmet(&self) -> Vec<&'static str> {
        self.as_slice()
            .iter()
            .zip(Self::NAMES)
            .filter(|(check, _)| !check.is_acceptable())
            .map(|(_, name)| name)
            .collect()
    }

    fn not_applicable_evidence_refs(&self) -> Vec<(&'static str, &str)> {
        self.as_slice()
            .iter()
            .zip(Self::NAMES)
            .filter_map(|(check, name)| {
                check
                    .outcome
                    .is_not_applicable()
                    .then_some((name, check.evidence_ref.as_str()))
            })
            .collect()
    }

    /// True only when every required check is acceptable with non-empty evidence.
    #[must_use]
    pub fn all_acceptable(&self) -> bool {
        self.unmet().is_empty()
    }
}

/// Machine-readable source-proof claim limitation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceProofClaimLimit {
    pub id: String,
    pub severity: String,
    pub claim: String,
    pub reason: String,
    pub evidence_ref: String,
}

impl SourceProofClaimLimit {
    #[must_use]
    pub(crate) fn to_result_contract_claim_limit(&self) -> String {
        format!(
            "source_proof_claim_limit id={} severity={} claim={} reason={} evidence_ref={}",
            self.id, self.severity, self.claim, self.reason, self.evidence_ref
        )
    }
}

/// One point-in-time source component used to build a cross-market signal.
///
/// Component roles are source roles, not venue identities. Concrete role names,
/// venues, and providers stay in TOML source bindings and source-proof evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CrossMarketJoinComponent {
    pub role: String,
    pub source_binding: String,
    pub source_proof_id: String,
    pub source_proof_version: u32,
    pub event_time_utc: String,
    pub available_at_utc: String,
    pub join_time_utc: String,
}

/// Thin L2 replay evidence pointers.
///
/// `L2_REPLAY` requires at least one of these fields to identify historical
/// source-order-preserving deltas or snapshots with cadence sufficient for the
/// strategy decision interval. L2 proofs also require an explicit tick-size
/// policy pointer: either a source-proof-bound no-tick-size-change universe or
/// an approved timed instrument-epoch replay mechanism. Non-L2 proofs still
/// carry this object with all fields empty so the schema surface is explicit
/// and stable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct L2ReplayEvidence {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order_book_delta_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sufficient_snapshot_cadence_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub no_tick_size_change_universe_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timed_instrument_epoch_replay_ref: Option<String>,
}

impl L2ReplayEvidence {
    fn has_replay_evidence(&self) -> bool {
        self.order_book_delta_ref
            .as_ref()
            .is_some_and(|value| !value.trim().is_empty())
            || self
                .sufficient_snapshot_cadence_ref
                .as_ref()
                .is_some_and(|value| !value.trim().is_empty())
    }

    fn has_tick_size_policy_evidence(&self) -> bool {
        self.no_tick_size_change_universe_ref
            .as_ref()
            .is_some_and(|value| !value.trim().is_empty())
            || self
                .timed_instrument_epoch_replay_ref
                .as_ref()
                .is_some_and(|value| !value.trim().is_empty())
    }
}

/// Inclusive-start, exclusive-end UTC time range (RFC 3339 strings).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimeRange {
    pub start_utc: String,
    pub end_utc: String,
}

/// Structured run-scope summary for the source proof's accepted manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcceptanceScope {
    pub planned_objects: u64,
    pub completed_objects: u64,
    pub failed_objects: u64,
    pub skipped_objects: u64,
    pub accepted_bytes: u64,
    pub selector_scope_violations: u64,
}

/// A thin source-proof record per the `backfill-source-proof.v1` contract.
///
/// The report is a proof pointer and claim-limit gate, not a data store: it
/// references raw/schema samples, hashes, and check evidence under the
/// configured `artifact_root` rather than embedding payloads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceProofReport {
    pub source_proof_id: String,
    pub source_proof_version: u32,
    pub contract_version: String,
    pub schema_version: String,
    pub status: SourceProofStatus,
    pub source_binding: String,
    pub venue: String,
    pub product_family: String,
    pub product_category: String,
    pub table_family: String,
    pub evidence_state: EvidenceState,
    pub source_candidate_class: SourceCandidateClass,
    pub source_selection_status: SourceSelectionStatus,
    #[serde(default = "default_source_proof_usage_scope")]
    pub usage_scope: SourceProofUsageScope,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub official_free_gap_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paid_vendor_gap_ref: Option<String>,
    pub fixture_type: FixtureType,
    pub requested_time_range: TimeRange,
    pub coverage_time_range: TimeRange,
    pub instrument_universe_id: String,
    pub raw_sample_uri: String,
    pub raw_sample_hash: String,
    pub schema_sample_uri: String,
    pub schema_sample_hash: String,
    pub license_ref: String,
    #[serde(default)]
    pub license_scope: LicenseScope,
    pub retention_ref: String,
    pub cost_ref: String,
    pub nt_mapping_status: NtMappingStatus,
    pub fidelity_class: SourceProofFidelityClass,
    pub l2_replay_evidence: L2ReplayEvidence,
    pub forbidden_claims: Vec<String>,
    /// Structured limitation records backing `forbidden_claims`.
    #[serde(default)]
    pub claim_limits: Vec<SourceProofClaimLimit>,
    /// Point-in-time component proofs for cross-market signal families.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cross_market_components: Vec<CrossMarketJoinComponent>,
    /// Structured manifest/run summary proving acceptance is bounded by object
    /// counts, byte counts, failures, skips, and selector-scope checks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acceptance_scope: Option<AcceptanceScope>,
    /// Required when gaps are tolerated; empty string when not applicable.
    pub gap_policy_id: String,
    pub required_checks: RequiredChecks,
    /// Acceptance provenance — present only on accepted reports.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub acceptance_mode: Option<AcceptanceMode>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub accepted_by: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub accepted_at: Option<String>,
    /// Prior accepted proof id this version supersedes, if any.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub supersedes_source_proof_id: Option<String>,
}

/// Why a candidate proof cannot be accepted, or why a dataset is inadmissible.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AcceptanceError {
    /// A required identity/evidence field was empty.
    MissingField(&'static str),
    /// A field was present where this proof status must not carry it.
    UnexpectedField {
        field: &'static str,
        reason: &'static str,
    },
    /// A version field does not equal the contract/schema version this module
    /// implements, so the proof was written against a different contract.
    UnexpectedVersion {
        field: &'static str,
        expected: &'static str,
        actual: String,
    },
    /// The proof's NautilusTrader catalog-mapping status is not `Accepted`.
    NtMappingNotAccepted(NtMappingStatus),
    /// The license scope cannot authorize BTE canonical/backtest input.
    LicenseScopeNotPermitted(LicenseScope),
    /// One or more required checks did not pass.
    UnmetChecks(Vec<&'static str>),
    /// The lower-fidelity source cannot carry an execution-quality claim.
    ForbiddenClaimMissing,
    /// A structured claim-limit row is missing or malformed.
    InvalidClaimLimit {
        field: &'static str,
        reason: &'static str,
    },
    /// A cross-market signal source lacks point-in-time component proof.
    InvalidCrossMarketJoin { field: &'static str, reason: String },
    /// Required-check evidence expired before the proof coverage ended.
    ExpiredRequiredCheck {
        check: &'static str,
        expires_at_utc: String,
        required_through_utc: String,
    },
    /// A not-applicable required check lacks a matching structured claim limit.
    NotApplicableCheckMissingClaimLimit { check: &'static str },
    /// The proof referenced by the dataset is not accepted.
    ProofNotAccepted(SourceProofStatus),
    /// A rejected proof cannot satisfy acceptance invariants.
    ProofRejected,
    /// The manifest payload record lacks a required field.
    ManifestRecordIncomplete(&'static str),
    /// The verified object hash does not match the manifest record hash.
    ContentHashMismatch { expected: String, actual: String },
    /// The selected manifest object is not the raw sample object named by the
    /// accepted source proof.
    RawSampleUriMismatch { expected: String, actual: String },
    /// A raw/sample artifact URI does not point at staged S3 object storage.
    InvalidStagedUri { field: &'static str, uri: String },
    /// The selected object lies outside the proof's proven coverage window.
    OutsideCoverage { object_date: String },
    /// The proof coverage window is not contained by the requested window.
    CoverageOutsideRequested,
    /// Acceptance was attempted on a proof whose status is not `Pending`, so an
    /// already-rejected (or already-accepted) record cannot be silently promoted.
    NotPending(SourceProofStatus),
    /// `supersedes_source_proof_id` references the proof's own id.
    SelfReferentialSupersede,
    /// A coverage bound (or the object archive date) is not a valid RFC 3339 /
    /// `YYYY-MM-DD` value, or the coverage window is inverted.
    MalformedCoverageBound { field: &'static str, value: String },
    /// The object's source provenance does not reference the proof's venue.
    SourceVenueMismatch { venue: String, source_url: String },
    /// The evidence state is not allowed for accepted canonical backfill input.
    EvidenceStateNotBackfillable(EvidenceState),
    /// Source selection status is not eligible for acceptance.
    SourceSelectionNotAccepted(SourceSelectionStatus),
    /// One-off bootstrap/backfill data cannot be promoted into canonical input.
    OneOffBackfillDataNotCanonical,
    /// The source proof disagrees with the configured source-binding metadata.
    SourceBindingMismatch {
        field: &'static str,
        expected: String,
        actual: String,
    },
    /// The source proof references no configured source-binding registry row.
    UnknownSourceBinding {
        source_binding: String,
        venue: String,
    },
    /// The structured manifest/run scope summary is not admissible for an
    /// accepted source proof.
    InvalidAcceptanceScope {
        field: &'static str,
        reason: &'static str,
    },
}

impl std::fmt::Display for AcceptanceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingField(field) => write!(f, "missing required field: {field}"),
            Self::UnexpectedField { field, reason } => {
                write!(f, "unexpected field {field}: {reason}")
            }
            Self::UnexpectedVersion {
                field,
                expected,
                actual,
            } => write!(
                f,
                "unexpected {field}: expected {expected:?}, got {actual:?}"
            ),
            Self::NtMappingNotAccepted(status) => {
                write!(f, "nt_mapping_status is not accepted (status: {status:?})")
            }
            Self::LicenseScopeNotPermitted(scope) => {
                write!(
                    f,
                    "license_scope {scope:?} is not permitted for BTE canonical/backtest input"
                )
            }
            Self::UnmetChecks(checks) => write!(f, "unmet required checks: {}", checks.join(", ")),
            Self::ForbiddenClaimMissing => {
                write!(f, "non-L2 fidelity requires explicit forbidden claims")
            }
            Self::InvalidClaimLimit { field, reason } => {
                write!(f, "claim_limits.{field} {reason}")
            }
            Self::InvalidCrossMarketJoin { field, reason } => {
                write!(f, "{field} {reason}")
            }
            Self::ExpiredRequiredCheck {
                check,
                expires_at_utc,
                required_through_utc,
            } => write!(
                f,
                "required_checks.{check} evidence expired at {expires_at_utc:?}; required through {required_through_utc:?}"
            ),
            Self::NotApplicableCheckMissingClaimLimit { check } => write!(
                f,
                "required_checks.{check} not_applicable requires matching claim_limits.evidence_ref"
            ),
            Self::ProofNotAccepted(status) => {
                write!(f, "source proof is not accepted (status: {status:?})")
            }
            Self::ProofRejected => write!(f, "rejected source proof cannot be accepted"),
            Self::ManifestRecordIncomplete(field) => {
                write!(f, "ingest manifest record incomplete: {field}")
            }
            Self::ContentHashMismatch { expected, actual } => {
                write!(
                    f,
                    "content hash mismatch: expected {expected}, got {actual}"
                )
            }
            Self::RawSampleUriMismatch { expected, actual } => {
                write!(
                    f,
                    "raw_sample_uri mismatch: expected proof raw_sample_uri {expected:?}, got manifest s3_uri {actual:?}"
                )
            }
            Self::InvalidStagedUri { field, uri } => {
                write!(f, "{field} must be a staged s3:// URI, got {uri:?}")
            }
            Self::OutsideCoverage { object_date } => {
                write!(
                    f,
                    "object date {object_date} outside proven coverage window"
                )
            }
            Self::CoverageOutsideRequested => {
                write!(f, "coverage window extends outside requested proof window")
            }
            Self::NotPending(status) => {
                write!(
                    f,
                    "source proof status is {status:?}, expected pending for acceptance"
                )
            }
            Self::SelfReferentialSupersede => {
                write!(
                    f,
                    "supersedes_source_proof_id must not reference the proof's own id"
                )
            }
            Self::MalformedCoverageBound { field, value } => {
                write!(f, "malformed coverage bound {field}: {value:?}")
            }
            Self::SourceVenueMismatch { venue, source_url } => {
                write!(
                    f,
                    "object source_url {source_url:?} does not reference proof venue {venue:?}"
                )
            }
            Self::EvidenceStateNotBackfillable(evidence_state) => {
                write!(
                    f,
                    "evidence_state {evidence_state:?} is not backfillable for accepted source proof"
                )
            }
            Self::SourceSelectionNotAccepted(status) => {
                write!(
                    f,
                    "source_selection_status {status:?} is not accepted for canonical source proof"
                )
            }
            Self::OneOffBackfillDataNotCanonical => {
                write!(
                    f,
                    "one_off_backfill_data source proofs cannot be accepted as canonical source proof input"
                )
            }
            Self::SourceBindingMismatch {
                field,
                expected,
                actual,
            } => {
                write!(
                    f,
                    "source_binding {field} mismatch: expected {expected:?}, got {actual:?}"
                )
            }
            Self::UnknownSourceBinding {
                source_binding,
                venue,
            } => {
                write!(
                    f,
                    "source_binding {source_binding:?} for venue {venue:?} is not configured in the registry"
                )
            }
            Self::InvalidAcceptanceScope { field, reason } => {
                write!(f, "acceptance_scope.{field} {reason}")
            }
        }
    }
}

impl std::error::Error for AcceptanceError {}

impl SourceProofReport {
    fn check_required_identity(&self) -> Result<(), AcceptanceError> {
        let required: [(&'static str, &str); 16] = [
            ("source_proof_id", &self.source_proof_id),
            ("contract_version", &self.contract_version),
            ("schema_version", &self.schema_version),
            ("source_binding", &self.source_binding),
            ("venue", &self.venue),
            ("product_family", &self.product_family),
            ("product_category", &self.product_category),
            ("table_family", &self.table_family),
            ("instrument_universe_id", &self.instrument_universe_id),
            ("raw_sample_uri", &self.raw_sample_uri),
            ("raw_sample_hash", &self.raw_sample_hash),
            ("schema_sample_uri", &self.schema_sample_uri),
            ("schema_sample_hash", &self.schema_sample_hash),
            ("license_ref", &self.license_ref),
            ("retention_ref", &self.retention_ref),
            ("cost_ref", &self.cost_ref),
        ];
        for (name, value) in required {
            if value.trim().is_empty() {
                return Err(AcceptanceError::MissingField(name));
            }
        }
        if self.source_proof_version == 0 {
            return Err(AcceptanceError::MissingField("source_proof_version"));
        }
        // Versions must match the contract/schema this module implements, so a
        // proof written against a different contract cannot be accepted here.
        if self.contract_version != CONTRACT_VERSION {
            return Err(AcceptanceError::UnexpectedVersion {
                field: "contract_version",
                expected: CONTRACT_VERSION,
                actual: self.contract_version.clone(),
            });
        }
        if self.schema_version != SOURCE_PROOF_SCHEMA_VERSION {
            return Err(AcceptanceError::UnexpectedVersion {
                field: "schema_version",
                expected: SOURCE_PROOF_SCHEMA_VERSION,
                actual: self.schema_version.clone(),
            });
        }
        Ok(())
    }

    /// Evaluate whether this candidate proof may be accepted.
    ///
    /// Acceptance requires every required identity field to be present, every
    /// required check to pass with evidence, and — for any non-`L2_REPLAY`
    /// fidelity — at least one explicit forbidden claim so weaker data cannot
    /// silently carry execution-quality claims.
    ///
    /// # Errors
    ///
    /// Returns the first blocking [`AcceptanceError`].
    pub fn evaluate_acceptance(&self) -> Result<(), AcceptanceError> {
        self.evaluate_acceptance_with_registry(&committed_source_binding_registry())
    }

    pub fn evaluate_acceptance_with_registry(
        &self,
        registry: &SourceBindingRegistry,
    ) -> Result<(), AcceptanceError> {
        if self.status == SourceProofStatus::Rejected {
            return Err(AcceptanceError::ProofRejected);
        }
        self.check_required_identity()?;
        ensure_license_scope_permits_bte_use(self.license_scope)?;
        validate_acceptance_provenance_shape(self)?;
        ensure_staged_s3_uri("raw_sample_uri", &self.raw_sample_uri)?;
        ensure_staged_s3_uri("schema_sample_uri", &self.schema_sample_uri)?;
        ensure_backfillable_evidence_state(self.evidence_state)?;
        let source_binding = ensure_source_binding_metadata_matches(self, registry)?;
        validate_source_selection(self)?;
        validate_l2_replay_evidence(self)?;
        let acceptance_scope = self
            .acceptance_scope
            .as_ref()
            .ok_or(AcceptanceError::MissingField("acceptance_scope"))?;
        validate_acceptance_scope(acceptance_scope, &self.gap_policy_id)?;
        ensure_coverage_within_requested(&self.requested_time_range, &self.coverage_time_range)?;
        if self.nt_mapping_status != NtMappingStatus::Accepted {
            return Err(AcceptanceError::NtMappingNotAccepted(
                self.nt_mapping_status,
            ));
        }
        let unmet = self.required_checks.unmet();
        if !unmet.is_empty() {
            return Err(AcceptanceError::UnmetChecks(unmet));
        }
        if self.fidelity_class != SourceProofFidelityClass::L2Replay
            && self.forbidden_claims.is_empty()
        {
            return Err(AcceptanceError::ForbiddenClaimMissing);
        }
        validate_claim_limits(self)?;
        validate_not_applicable_required_checks(self)?;
        validate_required_check_expiry(self)?;
        validate_cross_market_components(
            self,
            &source_binding.required_cross_market_component_roles,
        )?;
        // When the proof claims to supersede a prior proof, that reference must be
        // a real, distinct id — not blank and not the proof's own id.
        if let Some(superseded) = &self.supersedes_source_proof_id {
            if superseded.trim().is_empty() {
                return Err(AcceptanceError::MissingField("supersedes_source_proof_id"));
            }
            if superseded == &self.source_proof_id {
                return Err(AcceptanceError::SelfReferentialSupersede);
            }
        }
        Ok(())
    }

    /// Accept this candidate proof, stamping acceptance provenance and marking
    /// the record [`SourceProofStatus::Accepted`].
    ///
    /// # Errors
    ///
    /// Returns an [`AcceptanceError`] if [`Self::evaluate_acceptance`] fails;
    /// the record is left unchanged.
    pub fn accept(
        self,
        mode: AcceptanceMode,
        accepted_by: impl Into<String>,
        accepted_at_utc: impl Into<String>,
    ) -> Result<Self, AcceptanceError> {
        self.accept_with_registry(
            &committed_source_binding_registry(),
            mode,
            accepted_by,
            accepted_at_utc,
        )
    }

    pub fn accept_with_registry(
        mut self,
        registry: &SourceBindingRegistry,
        mode: AcceptanceMode,
        accepted_by: impl Into<String>,
        accepted_at_utc: impl Into<String>,
    ) -> Result<Self, AcceptanceError> {
        // Only a pending proof may be promoted: an already-rejected (or
        // already-accepted) record must not be silently re-promoted.
        if self.status != SourceProofStatus::Pending {
            return Err(AcceptanceError::NotPending(self.status));
        }
        self.evaluate_acceptance_with_registry(registry)?;
        // Acceptance provenance is mandatory: an accepted record must record who
        // accepted it and when, or the acceptance is unattributable.
        let accepted_by = accepted_by.into();
        let accepted_at_utc = accepted_at_utc.into();
        if accepted_by.trim().is_empty() {
            return Err(AcceptanceError::MissingField("accepted_by"));
        }
        if accepted_at_utc.trim().is_empty() {
            return Err(AcceptanceError::MissingField("accepted_at"));
        }
        self.status = SourceProofStatus::Accepted;
        self.acceptance_mode = Some(mode);
        self.accepted_by = Some(accepted_by);
        self.accepted_at = Some(accepted_at_utc);
        Ok(self)
    }

    #[must_use]
    pub fn is_accepted(&self) -> bool {
        self.status == SourceProofStatus::Accepted
    }
}

/// One payload record from an ingest manifest, tying a staged object to its
/// source, content hash, byte count, and schema header.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IngestManifestObjectRecord {
    pub s3_uri: String,
    pub source_url: String,
    pub sha256: String,
    pub bytes: u64,
    /// The archive partition date (`YYYY-MM-DD`) for the object.
    pub archive_date: String,
    /// Parsed header column names captured by the ingest run.
    pub schema_columns: Vec<String>,
}

impl IngestManifestObjectRecord {
    fn check_complete(&self) -> Result<(), AcceptanceError> {
        if self.sha256.trim().is_empty() {
            return Err(AcceptanceError::ManifestRecordIncomplete("sha256"));
        }
        if self.s3_uri.trim().is_empty() {
            return Err(AcceptanceError::ManifestRecordIncomplete("s3_uri"));
        }
        if self.bytes == 0 {
            return Err(AcceptanceError::ManifestRecordIncomplete("bytes"));
        }
        if self.schema_columns.is_empty() {
            return Err(AcceptanceError::ManifestRecordIncomplete("schema_columns"));
        }
        if self.archive_date.trim().is_empty() {
            return Err(AcceptanceError::ManifestRecordIncomplete("archive_date"));
        }
        Ok(())
    }
}

/// An object admitted as canonical backtest input: an accepted proof plus a
/// hash-verified, in-coverage manifest object record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcceptedDataset {
    pub(crate) source_proof_id: String,
    pub(crate) source_proof_version: u32,
    pub(crate) source_binding: String,
    pub(crate) venue: String,
    pub(crate) product_family: String,
    pub(crate) product_category: String,
    pub(crate) fixture_type: FixtureType,
    pub(crate) table_family: String,
    pub(crate) instrument_universe_id: String,
    pub(crate) fidelity_class: SourceProofFidelityClass,
    pub(crate) forbidden_claims: Vec<String>,
    pub(crate) claim_limits: Vec<SourceProofClaimLimit>,
    pub(crate) acceptance_mode: AcceptanceMode,
    pub(crate) accepted_by: String,
    pub(crate) accepted_at: String,
    pub(crate) accepted_object_sha256: String,
    pub(crate) object: IngestManifestObjectRecord,
    _accepted_gate: AcceptedGate,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AcceptedGate;

#[cfg(test)]
pub(crate) fn synthetic_accepted_dataset_for_tests() -> AcceptedDataset {
    let object = IngestManifestObjectRecord {
        s3_uri: "s3://synthetic-artifacts/source-proofs/raw/object.csv.gz".to_string(),
        source_url: "https://source.example.test/spot/TESTPAIR/TESTPAIR_2026-03-01.csv.gz"
            .to_string(),
        sha256: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
        bytes: 1,
        archive_date: "2026-03-01".to_string(),
        schema_columns: vec!["id".to_string()],
    };
    let forbidden_claims = vec!["No execution-quality claims.".to_string()];
    let claim_limits = forbidden_claims
        .iter()
        .enumerate()
        .map(|(index, claim)| SourceProofClaimLimit {
            id: format!("claim-limit-{}", index + 1),
            severity: "blocking".to_string(),
            claim: claim.clone(),
            reason: "source fidelity does not prove this claim".to_string(),
            evidence_ref: "source-proof://synthetic/fidelity-class".to_string(),
        })
        .collect();

    AcceptedDataset {
        source_proof_id: "source-proof-synthetic-native-trades".to_string(),
        source_proof_version: 1,
        source_binding: "synthetic-native-trades".to_string(),
        venue: "synthetic-venue".to_string(),
        product_family: "spot".to_string(),
        product_category: "spot".to_string(),
        fixture_type: FixtureType::PerpsSpot,
        table_family: "trades".to_string(),
        instrument_universe_id: "synthetic-instrument-universe".to_string(),
        fidelity_class: SourceProofFidelityClass::TradeReplay,
        forbidden_claims,
        claim_limits,
        acceptance_mode: AcceptanceMode::Manual,
        accepted_by: "operator".to_string(),
        accepted_at: "2026-06-02T00:00:00Z".to_string(),
        accepted_object_sha256: object.sha256.clone(),
        object,
        _accepted_gate: AcceptedGate,
    }
}

impl AcceptedDataset {
    #[must_use]
    pub(crate) fn result_contract_claim_limits(&self) -> Vec<String> {
        self.claim_limits
            .iter()
            .map(SourceProofClaimLimit::to_result_contract_claim_limit)
            .collect()
    }
}

/// Select an accepted dataset for backtest input.
///
/// Rejects anything that lacks an accepted source proof, a complete manifest
/// record (hash, bytes, schema, date), a verified content hash, or coverage of
/// the object date by the proof.
///
/// `verified_object_sha256` must be computed from the actual object bytes by the
/// caller; this function does not trust the manifest hash without an independent
/// content hash to compare against.
///
/// # Errors
///
/// Returns the first blocking [`AcceptanceError`].
pub fn select_accepted_dataset(
    proof: &SourceProofReport,
    object: &IngestManifestObjectRecord,
    verified_object_sha256: &str,
) -> Result<AcceptedDataset, AcceptanceError> {
    select_accepted_dataset_with_registry(
        proof,
        object,
        verified_object_sha256,
        &committed_source_binding_registry(),
    )
}

pub fn select_accepted_dataset_with_registry(
    proof: &SourceProofReport,
    object: &IngestManifestObjectRecord,
    verified_object_sha256: &str,
    registry: &SourceBindingRegistry,
) -> Result<AcceptedDataset, AcceptanceError> {
    if !proof.is_accepted() {
        return Err(AcceptanceError::ProofNotAccepted(proof.status));
    }
    // Defence in depth: re-evaluate the acceptance invariants even for a record
    // that already claims accepted status, so a hand-edited record cannot slip
    // through.
    proof.evaluate_acceptance_with_registry(registry)?;
    object.check_complete()?;
    ensure_staged_s3_uri("raw_sample_uri", &proof.raw_sample_uri)?;
    ensure_staged_s3_uri("s3_uri", &object.s3_uri)?;
    if proof.raw_sample_uri.trim() != object.s3_uri.trim() {
        return Err(AcceptanceError::RawSampleUriMismatch {
            expected: proof.raw_sample_uri.clone(),
            actual: object.s3_uri.clone(),
        });
    }
    let acceptance_scope = proof
        .acceptance_scope
        .as_ref()
        .ok_or(AcceptanceError::MissingField("acceptance_scope"))?;
    if object.bytes > acceptance_scope.accepted_bytes {
        return Err(AcceptanceError::InvalidAcceptanceScope {
            field: "accepted_bytes",
            reason: "must be at least selected object bytes",
        });
    }

    if verified_object_sha256 != object.sha256 {
        return Err(AcceptanceError::ContentHashMismatch {
            expected: object.sha256.clone(),
            actual: verified_object_sha256.to_string(),
        });
    }
    if object.sha256 != proof.raw_sample_hash {
        return Err(AcceptanceError::ContentHashMismatch {
            expected: proof.raw_sample_hash.clone(),
            actual: object.sha256.clone(),
        });
    }

    // Bind the object to the proof's source: the object's own provenance URL
    // must use the HTTPS host declared by the source-binding registry. This is
    // stricter than venue-label inference and avoids accepting arbitrary TLDs.
    if !source_url_matches_declared_source(
        &object.source_url,
        &proof.source_binding,
        &proof.venue,
        registry,
    ) {
        return Err(AcceptanceError::SourceVenueMismatch {
            venue: proof.venue.clone(),
            source_url: object.source_url.clone(),
        });
    }

    if !coverage_contains_date(
        &object.archive_date,
        &proof.coverage_time_range.start_utc,
        &proof.coverage_time_range.end_utc,
    )? {
        return Err(AcceptanceError::OutsideCoverage {
            object_date: object.archive_date.clone(),
        });
    }

    let acceptance_mode = proof
        .acceptance_mode
        .ok_or(AcceptanceError::MissingField("acceptance_mode"))?;
    let accepted_by = proof
        .accepted_by
        .as_ref()
        .filter(|value| !value.trim().is_empty())
        .ok_or(AcceptanceError::MissingField("accepted_by"))?;
    let accepted_at = proof
        .accepted_at
        .as_ref()
        .filter(|value| !value.trim().is_empty())
        .ok_or(AcceptanceError::MissingField("accepted_at"))?;

    Ok(AcceptedDataset {
        source_proof_id: proof.source_proof_id.clone(),
        source_proof_version: proof.source_proof_version,
        source_binding: proof.source_binding.clone(),
        venue: proof.venue.clone(),
        product_family: proof.product_family.clone(),
        product_category: proof.product_category.clone(),
        fixture_type: proof.fixture_type,
        table_family: proof.table_family.clone(),
        instrument_universe_id: proof.instrument_universe_id.clone(),
        fidelity_class: proof.fidelity_class,
        forbidden_claims: proof.forbidden_claims.clone(),
        claim_limits: proof.claim_limits.clone(),
        acceptance_mode,
        accepted_by: accepted_by.clone(),
        accepted_at: accepted_at.clone(),
        accepted_object_sha256: object.sha256.clone(),
        object: object.clone(),
        _accepted_gate: AcceptedGate,
    })
}

fn ensure_staged_s3_uri(field: &'static str, uri: &str) -> Result<(), AcceptanceError> {
    let uri = uri.trim();
    if uri.starts_with("s3://") {
        Ok(())
    } else {
        Err(AcceptanceError::InvalidStagedUri {
            field,
            uri: uri.to_string(),
        })
    }
}

fn source_url_matches_declared_source(
    source_url: &str,
    source_binding: &str,
    venue: &str,
    registry: &SourceBindingRegistry,
) -> bool {
    let Some(config) = registry.source_binding_config(source_binding, venue) else {
        return false;
    };
    if source_url.contains(['{', '}']) {
        return false;
    }
    let Some(declared) = https_url_parts(&config.source_uri) else {
        return false;
    };
    let Some(object) = https_url_parts(source_url) else {
        return false;
    };
    object.host.eq_ignore_ascii_case(declared.host)
        && template_remainder_matches(declared.remainder, object.remainder)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct HttpsUrlParts<'a> {
    host: &'a str,
    remainder: &'a str,
}

fn https_url_parts(source_url: &str) -> Option<HttpsUrlParts<'_>> {
    let (scheme, after_scheme) = source_url.split_once("://")?;
    if !scheme.eq_ignore_ascii_case("https") {
        return None;
    }
    let without_fragment = after_scheme.split('#').next().unwrap_or_default();
    let remainder_start = without_fragment
        .find(['/', '?'])
        .unwrap_or(without_fragment.len());
    let authority = without_fragment[..remainder_start].trim();
    let remainder = &without_fragment[remainder_start..];
    let host = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host)
        .split(':')
        .next()
        .unwrap_or_default()
        .trim_matches(['[', ']'])
        .trim_matches('.');
    if host.is_empty() {
        None
    } else {
        Some(HttpsUrlParts { host, remainder })
    }
}

fn template_remainder_matches(mut template: &str, mut actual: &str) -> bool {
    while let Some(open) = template.find('{') {
        let literal = &template[..open];
        if !actual.starts_with(literal) {
            return false;
        }
        actual = &actual[literal.len()..];
        let Some(close_after_open) = template[open + 1..].find('}') else {
            return false;
        };
        let close = open + 1 + close_after_open;
        if close == open + 1 {
            return false;
        }
        template = &template[close + 1..];
        let next_literal_end = template.find('{').unwrap_or(template.len());
        let next_literal = &template[..next_literal_end];
        if next_literal.is_empty() {
            return !actual.is_empty() && !actual.contains(['/', '?', '#']);
        }
        let Some(match_end) = actual.find(next_literal) else {
            return false;
        };
        let matched_placeholder = &actual[..match_end];
        if matched_placeholder.is_empty() || matched_placeholder.contains(['/', '?', '#']) {
            return false;
        }
        actual = &actual[match_end..];
    }
    actual == template
}

fn ensure_license_scope_permits_bte_use(scope: LicenseScope) -> Result<(), AcceptanceError> {
    match scope {
        LicenseScope::Commercial
        | LicenseScope::Enterprise
        | LicenseScope::Public
        | LicenseScope::Waived => Ok(()),
        LicenseScope::Personal | LicenseScope::Unknown => {
            Err(AcceptanceError::LicenseScopeNotPermitted(scope))
        }
    }
}

fn ensure_backfillable_evidence_state(
    evidence_state: EvidenceState,
) -> Result<(), AcceptanceError> {
    match evidence_state {
        EvidenceState::DirectlyBackfillable | EvidenceState::OwnerArchiveBackfillable => Ok(()),
        other => Err(AcceptanceError::EvidenceStateNotBackfillable(other)),
    }
}

fn validate_acceptance_scope(
    scope: &AcceptanceScope,
    gap_policy_id: &str,
) -> Result<(), AcceptanceError> {
    if scope.planned_objects == 0 {
        return Err(AcceptanceError::InvalidAcceptanceScope {
            field: "planned_objects",
            reason: "must be positive",
        });
    }
    if scope.completed_objects == 0 {
        return Err(AcceptanceError::InvalidAcceptanceScope {
            field: "completed_objects",
            reason: "must be positive",
        });
    }
    if scope.accepted_bytes == 0 {
        return Err(AcceptanceError::InvalidAcceptanceScope {
            field: "accepted_bytes",
            reason: "must be positive",
        });
    }
    if scope.failed_objects != 0 {
        return Err(AcceptanceError::InvalidAcceptanceScope {
            field: "failed_objects",
            reason: "must be zero",
        });
    }
    if scope.selector_scope_violations != 0 {
        return Err(AcceptanceError::InvalidAcceptanceScope {
            field: "selector_scope_violations",
            reason: "must be zero",
        });
    }
    let accounted_objects = scope
        .completed_objects
        .checked_add(scope.failed_objects)
        .and_then(|value| value.checked_add(scope.skipped_objects))
        .ok_or(AcceptanceError::InvalidAcceptanceScope {
            field: "planned_objects",
            reason: "must not overflow completed + failed + skipped object counts",
        })?;
    if accounted_objects != scope.planned_objects {
        return Err(AcceptanceError::InvalidAcceptanceScope {
            field: "planned_objects",
            reason: "must equal completed_objects + failed_objects + skipped_objects",
        });
    }
    if scope.skipped_objects != 0 && gap_policy_id.trim().is_empty() {
        return Err(AcceptanceError::InvalidAcceptanceScope {
            field: "skipped_objects",
            reason: "requires gap_policy_id",
        });
    }
    Ok(())
}

fn validate_acceptance_provenance_shape(proof: &SourceProofReport) -> Result<(), AcceptanceError> {
    let has_acceptance_provenance = proof.acceptance_mode.is_some()
        || proof.accepted_by.is_some()
        || proof.accepted_at.is_some();
    if proof.status == SourceProofStatus::Accepted {
        if proof.acceptance_mode.is_none() {
            return Err(AcceptanceError::MissingField("acceptance_mode"));
        }
        if proof
            .accepted_by
            .as_ref()
            .is_none_or(|value| value.trim().is_empty())
        {
            return Err(AcceptanceError::MissingField("accepted_by"));
        }
        if proof
            .accepted_at
            .as_ref()
            .is_none_or(|value| value.trim().is_empty())
        {
            return Err(AcceptanceError::MissingField("accepted_at"));
        }
    } else if has_acceptance_provenance {
        return Err(AcceptanceError::UnexpectedField {
            field: "acceptance_mode",
            reason: "acceptance provenance is only valid on accepted reports",
        });
    }
    Ok(())
}

fn validate_source_selection(proof: &SourceProofReport) -> Result<(), AcceptanceError> {
    if proof.usage_scope == SourceProofUsageScope::OneOffBackfillData {
        return Err(AcceptanceError::OneOffBackfillDataNotCanonical);
    }

    match proof.source_selection_status {
        SourceSelectionStatus::AcceptedForRequiredFidelity
        | SourceSelectionStatus::AcceptedLowerFidelity => {}
        status => return Err(AcceptanceError::SourceSelectionNotAccepted(status)),
    }

    if matches!(
        proof.source_candidate_class,
        SourceCandidateClass::PaidVendor | SourceCandidateClass::ForwardCapture
    ) && proof
        .official_free_gap_ref
        .as_ref()
        .is_none_or(|value| value.trim().is_empty())
    {
        return Err(AcceptanceError::MissingField("official_free_gap_ref"));
    }
    if proof.source_candidate_class == SourceCandidateClass::ForwardCapture
        && proof
            .paid_vendor_gap_ref
            .as_ref()
            .is_none_or(|value| value.trim().is_empty())
    {
        return Err(AcceptanceError::MissingField("paid_vendor_gap_ref"));
    }
    Ok(())
}

fn validate_l2_replay_evidence(proof: &SourceProofReport) -> Result<(), AcceptanceError> {
    if proof.fidelity_class != SourceProofFidelityClass::L2Replay {
        return Ok(());
    }
    if !proof.l2_replay_evidence.has_replay_evidence() {
        return Err(AcceptanceError::MissingField("l2_replay_evidence"));
    }
    if !proof.l2_replay_evidence.has_tick_size_policy_evidence() {
        return Err(AcceptanceError::MissingField(
            "l2_replay_evidence.tick_size_policy",
        ));
    }
    Ok(())
}

fn validate_claim_limits(proof: &SourceProofReport) -> Result<(), AcceptanceError> {
    for limit in &proof.claim_limits {
        if limit.id.trim().is_empty() {
            return Err(AcceptanceError::InvalidClaimLimit {
                field: "id",
                reason: "must not be empty",
            });
        }
        if limit.severity.trim().is_empty() {
            return Err(AcceptanceError::InvalidClaimLimit {
                field: "severity",
                reason: "must not be empty",
            });
        }
        if limit.claim.trim().is_empty() {
            return Err(AcceptanceError::InvalidClaimLimit {
                field: "claim",
                reason: "must not be empty",
            });
        }
        if limit.reason.trim().is_empty() {
            return Err(AcceptanceError::InvalidClaimLimit {
                field: "reason",
                reason: "must not be empty",
            });
        }
        if limit.evidence_ref.trim().is_empty() {
            return Err(AcceptanceError::InvalidClaimLimit {
                field: "evidence_ref",
                reason: "must not be empty",
            });
        }
    }

    if proof.claim_limits.is_empty()
        && (proof.fidelity_class != SourceProofFidelityClass::L2Replay
            || !proof.forbidden_claims.is_empty())
    {
        return Err(AcceptanceError::InvalidClaimLimit {
            field: "claim_limits",
            reason: "must not be empty when forbidden_claims are present",
        });
    }
    for forbidden_claim in &proof.forbidden_claims {
        if !proof
            .claim_limits
            .iter()
            .any(|limit| limit.claim == *forbidden_claim)
        {
            return Err(AcceptanceError::InvalidClaimLimit {
                field: "claim",
                reason: "must cover every forbidden_claims entry",
            });
        }
    }
    Ok(())
}

fn validate_not_applicable_required_checks(
    proof: &SourceProofReport,
) -> Result<(), AcceptanceError> {
    for (check_name, evidence_ref) in proof.required_checks.not_applicable_evidence_refs() {
        if !proof
            .claim_limits
            .iter()
            .any(|limit| limit.evidence_ref == evidence_ref)
        {
            return Err(AcceptanceError::NotApplicableCheckMissingClaimLimit { check: check_name });
        }
    }
    Ok(())
}

fn validate_required_check_expiry(proof: &SourceProofReport) -> Result<(), AcceptanceError> {
    let coverage_end =
        coverage_bound_nanos(&proof.coverage_time_range.end_utc, "coverage end_utc")?;
    for (check, check_name) in proof
        .required_checks
        .as_slice()
        .into_iter()
        .zip(RequiredChecks::NAMES)
    {
        let Some(expires_at_utc) = &check.expires_at_utc else {
            continue;
        };
        let expires_at = coverage_bound_nanos(expires_at_utc, "required_checks.expires_at_utc")?;
        if expires_at < coverage_end {
            return Err(AcceptanceError::ExpiredRequiredCheck {
                check: check_name,
                expires_at_utc: expires_at_utc.clone(),
                required_through_utc: proof.coverage_time_range.end_utc.clone(),
            });
        }
    }
    Ok(())
}

fn validate_cross_market_components(
    proof: &SourceProofReport,
    required_roles: &[String],
) -> Result<(), AcceptanceError> {
    for required_role in required_roles {
        if !proof
            .cross_market_components
            .iter()
            .any(|component| component.role == *required_role)
        {
            return Err(AcceptanceError::InvalidCrossMarketJoin {
                field: "cross_market_components",
                reason: format!("missing required role {required_role:?}"),
            });
        }
    }

    let coverage_start =
        coverage_bound_nanos(&proof.coverage_time_range.start_utc, "coverage start_utc")?;
    let coverage_end =
        coverage_bound_nanos(&proof.coverage_time_range.end_utc, "coverage end_utc")?;
    for component in &proof.cross_market_components {
        validate_cross_market_component(component, coverage_start, coverage_end)?;
    }
    Ok(())
}

fn validate_cross_market_component(
    component: &CrossMarketJoinComponent,
    coverage_start: i64,
    coverage_end: i64,
) -> Result<(), AcceptanceError> {
    for (field, value) in [
        ("cross_market_components.role", component.role.as_str()),
        (
            "cross_market_components.source_binding",
            component.source_binding.as_str(),
        ),
        (
            "cross_market_components.source_proof_id",
            component.source_proof_id.as_str(),
        ),
        (
            "cross_market_components.event_time_utc",
            component.event_time_utc.as_str(),
        ),
        (
            "cross_market_components.available_at_utc",
            component.available_at_utc.as_str(),
        ),
        (
            "cross_market_components.join_time_utc",
            component.join_time_utc.as_str(),
        ),
    ] {
        if value.trim().is_empty() {
            return Err(AcceptanceError::InvalidCrossMarketJoin {
                field,
                reason: "must not be empty".to_string(),
            });
        }
    }
    if component.source_proof_version == 0 {
        return Err(AcceptanceError::InvalidCrossMarketJoin {
            field: "cross_market_components.source_proof_version",
            reason: "must not be zero".to_string(),
        });
    }

    let join_time = coverage_bound_nanos(
        &component.join_time_utc,
        "cross_market_components.join_time_utc",
    )?;
    if join_time < coverage_start || join_time >= coverage_end {
        return Err(AcceptanceError::InvalidCrossMarketJoin {
            field: "cross_market_components.join_time_utc",
            reason: "must be inside proof coverage window".to_string(),
        });
    }

    let event_time = coverage_bound_nanos(
        &component.event_time_utc,
        "cross_market_components.event_time_utc",
    )?;
    if event_time > join_time {
        return Err(AcceptanceError::InvalidCrossMarketJoin {
            field: "cross_market_components.event_time_utc",
            reason: "would future-leak after join_time_utc".to_string(),
        });
    }

    let available_at = coverage_bound_nanos(
        &component.available_at_utc,
        "cross_market_components.available_at_utc",
    )?;
    if available_at > join_time {
        return Err(AcceptanceError::InvalidCrossMarketJoin {
            field: "cross_market_components.available_at_utc",
            reason: "would future-leak after join_time_utc".to_string(),
        });
    }
    Ok(())
}

fn ensure_source_binding_metadata_matches(
    proof: &SourceProofReport,
    registry: &SourceBindingRegistry,
) -> Result<SourceBindingConfig, AcceptanceError> {
    let Some(config) = registry.source_binding_config(&proof.source_binding, &proof.venue) else {
        return Err(AcceptanceError::UnknownSourceBinding {
            source_binding: proof.source_binding.clone(),
            venue: proof.venue.clone(),
        });
    };
    if proof.product_family != config.product_family {
        return Err(AcceptanceError::SourceBindingMismatch {
            field: "product_family",
            expected: config.product_family,
            actual: proof.product_family.clone(),
        });
    }
    let Some(market_structure_fixture) = config.market_structure_fixture else {
        return Err(AcceptanceError::MissingField(
            "source_binding.market_structure_fixture",
        ));
    };
    ensure_bte_market_structure_fixture(market_structure_fixture)?;
    if proof.fixture_type != market_structure_fixture {
        return Err(AcceptanceError::SourceBindingMismatch {
            field: "market_structure_fixture",
            expected: fixture_type_label(market_structure_fixture).to_string(),
            actual: fixture_type_label(proof.fixture_type).to_string(),
        });
    }
    if proof.evidence_state != config.evidence_state {
        return Err(AcceptanceError::SourceBindingMismatch {
            field: "evidence_state",
            expected: format!("{:?}", config.evidence_state),
            actual: format!("{:?}", proof.evidence_state),
        });
    }
    if !config.table_families.is_empty()
        && !config
            .table_families
            .iter()
            .any(|table_family| table_family == &proof.table_family)
    {
        return Err(AcceptanceError::SourceBindingMismatch {
            field: "table_family",
            expected: config.table_families.join(","),
            actual: proof.table_family.clone(),
        });
    }
    Ok(config)
}

fn ensure_bte_market_structure_fixture(fixture_type: FixtureType) -> Result<(), AcceptanceError> {
    if matches!(
        fixture_type,
        FixtureType::BinaryOption | FixtureType::PerpsSpot
    ) {
        Ok(())
    } else {
        Err(AcceptanceError::SourceBindingMismatch {
            field: "market_structure_fixture",
            expected: "binary-option|perps-spot".to_string(),
            actual: fixture_type_label(fixture_type).to_string(),
        })
    }
}

fn fixture_type_label(fixture_type: FixtureType) -> &'static str {
    match fixture_type {
        FixtureType::BinaryOption => "binary-option",
        FixtureType::PredictionMarket => "prediction-market",
        FixtureType::PerpsSpot => "perps-spot",
        FixtureType::Options => "options",
        FixtureType::Mixed => "mixed",
    }
}

fn ensure_coverage_within_requested(
    requested: &TimeRange,
    coverage: &TimeRange,
) -> Result<(), AcceptanceError> {
    let requested_start = coverage_bound_nanos(&requested.start_utc, "requested start_utc")?;
    let requested_end = coverage_bound_nanos(&requested.end_utc, "requested end_utc")?;
    let coverage_start = coverage_bound_nanos(&coverage.start_utc, "coverage start_utc")?;
    let coverage_end = coverage_bound_nanos(&coverage.end_utc, "coverage end_utc")?;
    if requested_start > requested_end {
        return Err(AcceptanceError::MalformedCoverageBound {
            field: "requested window",
            value: format!("{}..{}", requested.start_utc, requested.end_utc),
        });
    }
    if coverage_start > coverage_end {
        return Err(AcceptanceError::MalformedCoverageBound {
            field: "coverage window",
            value: format!("{}..{}", coverage.start_utc, coverage.end_utc),
        });
    }
    if coverage_start < requested_start || coverage_end > requested_end {
        return Err(AcceptanceError::CoverageOutsideRequested);
    }
    Ok(())
}

fn coverage_bound_nanos(value: &str, field: &'static str) -> Result<i64, AcceptanceError> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .and_then(|dt| dt.timestamp_nanos_opt())
        .ok_or_else(|| AcceptanceError::MalformedCoverageBound {
            field,
            value: value.to_string(),
        })
}

/// True when `object_date` (`YYYY-MM-DD`) falls in `[start_utc, end_utc)`.
///
/// The bounds are full RFC 3339 timestamps; the start date is inclusive and the
/// end date exclusive (correct for day-partitioned archive objects). Errors
/// loudly on a malformed bound, a malformed object date, or an inverted window
/// rather than silently comparing partial strings.
fn coverage_contains_date(
    object_date: &str,
    start_utc: &str,
    end_utc: &str,
) -> Result<bool, AcceptanceError> {
    let object = NaiveDate::parse_from_str(object_date, "%Y-%m-%d").map_err(|_| {
        AcceptanceError::MalformedCoverageBound {
            field: "object archive_date",
            value: object_date.to_string(),
        }
    })?;
    let start = coverage_bound_date(start_utc, "coverage start_utc")?;
    let end = coverage_bound_date(end_utc, "coverage end_utc")?;
    if start > end {
        return Err(AcceptanceError::MalformedCoverageBound {
            field: "coverage window",
            value: format!("{start_utc}..{end_utc}"),
        });
    }
    Ok(object >= start && object < end)
}

/// Parse an RFC 3339 coverage bound into its UTC calendar date.
fn coverage_bound_date(value: &str, field: &'static str) -> Result<NaiveDate, AcceptanceError> {
    DateTime::parse_from_rfc3339(value)
        .map(|dt| dt.date_naive())
        .map_err(|_| AcceptanceError::MalformedCoverageBound {
            field,
            value: value.to_string(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn passing_checks() -> RequiredChecks {
        let evidence = "manifest://bybit-backfill-run-fdcc0758bbd03113";
        RequiredChecks {
            source_access: RequiredCheck::passed(evidence),
            license: RequiredCheck::passed("attestation://bybit-public-archive"),
            schema: RequiredCheck::passed("schema://id,timestamp,price,volume,side,rpi"),
            time_semantics: RequiredCheck::passed("ms_to_unix_nanos"),
            instrument_universe: RequiredCheck::passed("universe://bybit-spot"),
            coverage: RequiredCheck::passed(evidence),
            retention_freshness: RequiredCheck::passed("retention://bybit-public-archive-reviewed"),
            granularity: RequiredCheck::passed("native_trade_prints"),
            nt_mapping: RequiredCheck::passed("nt://TradeTick"),
            completeness: RequiredCheck::passed(evidence),
            cost: RequiredCheck::passed("cost://free-public-archive"),
            storage: RequiredCheck::passed("s3://bolt-parquet/.../source-proofs/"),
        }
    }

    fn accepted_scope() -> AcceptanceScope {
        AcceptanceScope {
            planned_objects: 1,
            completed_objects: 1,
            failed_objects: 0,
            skipped_objects: 0,
            accepted_bytes: 8505,
            selector_scope_violations: 0,
        }
    }

    fn claim_limits_for(claims: &[String]) -> Vec<SourceProofClaimLimit> {
        claims
            .iter()
            .enumerate()
            .map(|(index, claim)| SourceProofClaimLimit {
                id: format!("claim-limit-{}", index + 1),
                severity: "blocking".to_string(),
                claim: claim.clone(),
                reason: "source fidelity does not prove this claim".to_string(),
                evidence_ref: "source-proof://fidelity-class".to_string(),
            })
            .collect()
    }

    fn candidate_proof() -> SourceProofReport {
        let forbidden_claims = vec![
            "No execution-quality, queue-position, or order-book-liquidity claims.".to_string(),
        ];
        SourceProofReport {
            source_proof_id: "source-proof-bybit-spot-tick-trades".to_string(),
            source_proof_version: 1,
            contract_version: CONTRACT_VERSION.to_string(),
            schema_version: SOURCE_PROOF_SCHEMA_VERSION.to_string(),
            status: SourceProofStatus::Pending,
            source_binding: "bybit-spot-tick-trades".to_string(),
            venue: "bybit".to_string(),
            product_family: "spot".to_string(),
            product_category: "spot".to_string(),
            table_family: "trades".to_string(),
            evidence_state: EvidenceState::OwnerArchiveBackfillable,
            source_candidate_class: SourceCandidateClass::OfficialFree,
            source_selection_status: SourceSelectionStatus::AcceptedLowerFidelity,
            usage_scope: SourceProofUsageScope::CanonicalBackfillInput,
            official_free_gap_ref: None,
            paid_vendor_gap_ref: None,
            fixture_type: FixtureType::PerpsSpot,
            requested_time_range: TimeRange {
                start_utc: "2025-06-01T00:00:00Z".to_string(),
                end_utc: "2026-06-01T00:00:00Z".to_string(),
            },
            coverage_time_range: TimeRange {
                start_utc: "2026-03-01T00:00:00Z".to_string(),
                end_utc: "2026-03-02T00:00:00Z".to_string(),
            },
            instrument_universe_id: "bybit-spot-instruments-2026-03-01".to_string(),
            raw_sample_uri: "s3://bolt-parquet/.../symbol=BNBUSDC/object=d6af93.csv.gz".to_string(),
            raw_sample_hash: "d6af93305f3773d6c00b4f3c13ffaef54a573d62ce5e6a96649b06d82df04598"
                .to_string(),
            schema_sample_uri: "s3://bolt-parquet/.../schema-sample.json".to_string(),
            schema_sample_hash: "bf26db0b8fb8b62746b5724dccfb26a408d581f5598cb6be95c9173c8b1b5eed"
                .to_string(),
            license_ref: "https://public.bybit.com/ (attestation 2026-06-02)".to_string(),
            license_scope: LicenseScope::Public,
            retention_ref: "https://public.bybit.com/ (archive retention reviewed)".to_string(),
            cost_ref: "cost://free-public-archive".to_string(),
            nt_mapping_status: NtMappingStatus::Accepted,
            fidelity_class: SourceProofFidelityClass::TradeReplay,
            l2_replay_evidence: L2ReplayEvidence {
                order_book_delta_ref: None,
                sufficient_snapshot_cadence_ref: None,
                no_tick_size_change_universe_ref: None,
                timed_instrument_epoch_replay_ref: None,
            },
            forbidden_claims: forbidden_claims.clone(),
            claim_limits: claim_limits_for(&forbidden_claims),
            cross_market_components: Vec::new(),
            acceptance_scope: Some(accepted_scope()),
            gap_policy_id: String::new(),
            required_checks: passing_checks(),
            acceptance_mode: None,
            accepted_by: None,
            accepted_at: None,
            supersedes_source_proof_id: None,
        }
    }

    fn manifest_object() -> IngestManifestObjectRecord {
        IngestManifestObjectRecord {
            s3_uri: "s3://bolt-parquet/.../symbol=BNBUSDC/object=d6af93.csv.gz".to_string(),
            source_url: "https://public.bybit.com/spot/BNBUSDC/BNBUSDC_2026-03-01.csv.gz"
                .to_string(),
            sha256: "d6af93305f3773d6c00b4f3c13ffaef54a573d62ce5e6a96649b06d82df04598".to_string(),
            bytes: 8505,
            archive_date: "2026-03-01".to_string(),
            schema_columns: vec![
                "id".to_string(),
                "timestamp".to_string(),
                "price".to_string(),
                "volume".to_string(),
                "side".to_string(),
                "rpi".to_string(),
            ],
        }
    }

    fn source_binding_registry_without_market_structure_fixture() -> SourceBindingRegistry {
        SourceBindingRegistry::from_toml_str(
            r#"
[[source_binding]]
key = "bybit-spot-tick-trades"
venue = "bybit"
product_family = "spot"
source_uri = "https://public.bybit.com/spot/{symbol}/{symbol}_{dt}.csv.gz"
evidence_state = "owner_archive_backfillable"
table_families = ["trades"]
"#,
        )
        .expect("source binding registry parses")
    }

    fn source_binding_registry_with_market_structure_fixture(
        fixture_type: &str,
    ) -> SourceBindingRegistry {
        SourceBindingRegistry::from_toml_str(&format!(
            r#"
[[source_binding]]
key = "bybit-spot-tick-trades"
venue = "bybit"
product_family = "spot"
market_structure_fixture = "{fixture_type}"
source_uri = "https://public.bybit.com/spot/{{symbol}}/{{symbol}}_{{dt}}.csv.gz"
evidence_state = "owner_archive_backfillable"
table_families = ["trades"]
"#
        ))
        .expect("source binding registry parses")
    }

    fn kimchi_registry() -> SourceBindingRegistry {
        SourceBindingRegistry::from_toml_str(
            r#"
[[source_binding]]
key = "synthetic-kimchi-premium-signal"
venue = "synthetic-signal-source"
product_family = "cross_market_signal"
market_structure_fixture = "perps-spot"
source_uri = "https://signals.example.test/kimchi/{dt}.json"
evidence_state = "directly_backfillable"
table_families = ["signals"]
required_cross_market_component_roles = ["korean_spot", "reference_price", "fx_quote", "token_mapping"]
"#,
        )
        .expect("kimchi registry parses")
    }

    fn cross_market_registry_with_required_roles(roles: &[&str]) -> SourceBindingRegistry {
        let roles = roles
            .iter()
            .map(|role| format!(r#""{role}""#))
            .collect::<Vec<_>>()
            .join(", ");
        SourceBindingRegistry::from_toml_str(&format!(
            r#"
[[source_binding]]
key = "synthetic-custom-cross-market-signal"
venue = "synthetic-signal-source"
product_family = "cross_market_signal"
market_structure_fixture = "perps-spot"
source_uri = "https://signals.example.test/custom/{{dt}}.json"
evidence_state = "directly_backfillable"
table_families = ["signals"]
required_cross_market_component_roles = [{roles}]
"#
        ))
        .expect("cross-market registry parses")
    }

    fn kimchi_component(role: &str) -> CrossMarketJoinComponent {
        CrossMarketJoinComponent {
            role: role.to_string(),
            source_binding: format!("{role}-binding"),
            source_proof_id: format!("source-proof-{role}"),
            source_proof_version: 1,
            event_time_utc: "2026-03-01T00:00:00Z".to_string(),
            available_at_utc: "2026-03-01T00:00:00Z".to_string(),
            join_time_utc: "2026-03-01T00:00:00Z".to_string(),
        }
    }

    fn kimchi_signal_proof() -> SourceProofReport {
        let mut proof = candidate_proof();
        let forbidden_claims = vec!["No execution-quality or trade-replay claims.".to_string()];
        proof.source_proof_id = "source-proof-synthetic-kimchi-premium".to_string();
        proof.source_binding = "synthetic-kimchi-premium-signal".to_string();
        proof.venue = "synthetic-signal-source".to_string();
        proof.product_family = "cross_market_signal".to_string();
        proof.product_category = "kimchi-premium".to_string();
        proof.table_family = "signals".to_string();
        proof.evidence_state = EvidenceState::DirectlyBackfillable;
        proof.fidelity_class = SourceProofFidelityClass::SignalOnly;
        proof.forbidden_claims = forbidden_claims.clone();
        proof.claim_limits = claim_limits_for(&forbidden_claims);
        proof.raw_sample_uri = "s3://bolt-parquet/.../kimchi/raw/sample.json".to_string();
        proof.schema_sample_uri = "s3://bolt-parquet/.../kimchi/schema-sample.json".to_string();
        proof.cross_market_components = vec![
            kimchi_component("korean_spot"),
            kimchi_component("reference_price"),
            kimchi_component("fx_quote"),
            kimchi_component("token_mapping"),
        ];
        proof
    }

    fn custom_cross_market_signal_proof() -> SourceProofReport {
        let mut proof = kimchi_signal_proof();
        proof.source_proof_id = "source-proof-synthetic-custom-cross-market".to_string();
        proof.source_binding = "synthetic-custom-cross-market-signal".to_string();
        proof.product_category = "custom-cross-market-signal".to_string();
        proof.raw_sample_uri =
            "s3://bolt-parquet/.../custom-cross-market/raw/sample.json".to_string();
        proof.schema_sample_uri =
            "s3://bolt-parquet/.../custom-cross-market/schema-sample.json".to_string();
        proof.cross_market_components = vec![kimchi_component("primary_leg")];
        proof
    }

    #[test]
    fn automated_acceptance_rejects_failed_required_checks() {
        for (check_index, check_name) in RequiredChecks::NAMES.iter().enumerate() {
            let mut proof = candidate_proof();
            *proof.required_checks.as_mut_slice()[check_index] = RequiredCheck {
                outcome: CheckOutcome::Failed,
                evidence_ref: format!("failed://{}", check_name),
                expires_at_utc: None,
            };

            let err = proof
                .accept(
                    AcceptanceMode::Automated,
                    "automated-source-proof-gate",
                    "2026-06-02T00:00:00Z",
                )
                .unwrap_err();

            assert_eq!(err, AcceptanceError::UnmetChecks(vec![*check_name]));
        }
    }

    #[test]
    fn automated_acceptance_rejects_expired_required_check() {
        let mut proof = candidate_proof();
        proof.required_checks.license.expires_at_utc = Some("2026-03-01T23:59:59Z".to_string());

        let err = proof
            .accept(
                AcceptanceMode::Automated,
                "automated-source-proof-gate",
                "2026-06-02T00:00:00Z",
            )
            .unwrap_err();

        assert_eq!(
            err,
            AcceptanceError::ExpiredRequiredCheck {
                check: "license",
                expires_at_utc: "2026-03-01T23:59:59Z".to_string(),
                required_through_utc: "2026-03-02T00:00:00Z".to_string(),
            }
        );
    }

    #[test]
    fn acceptance_rejects_unknown_license_scope() {
        let mut proof = candidate_proof();
        proof.license_scope = LicenseScope::Unknown;

        let err = proof.evaluate_acceptance().unwrap_err();

        assert_eq!(
            err,
            AcceptanceError::LicenseScopeNotPermitted(LicenseScope::Unknown)
        );
    }

    #[test]
    fn acceptance_rejects_personal_license_scope_for_bte_input() {
        let mut proof = candidate_proof();
        proof.license_scope = LicenseScope::Personal;

        let err = proof.evaluate_acceptance().unwrap_err();

        assert_eq!(
            err,
            AcceptanceError::LicenseScopeNotPermitted(LicenseScope::Personal)
        );
    }

    #[test]
    fn kimchi_premium_source_proof_requires_configured_component_roles_without_venue_constants() {
        let registry = kimchi_registry();
        let mut proof = kimchi_signal_proof();
        proof
            .cross_market_components
            .retain(|component| component.role != "fx_quote");

        let err = proof
            .evaluate_acceptance_with_registry(&registry)
            .unwrap_err();

        assert!(
            err.to_string().contains("cross_market_components")
                && err.to_string().contains("fx_quote"),
            "{err}"
        );

        kimchi_signal_proof()
            .evaluate_acceptance_with_registry(&registry)
            .expect("configured cross-market roles should satisfy kimchi signal proof");
    }

    #[test]
    fn source_binding_registry_declares_required_cross_market_component_roles() {
        let registry = cross_market_registry_with_required_roles(&["primary_leg", "reference_leg"]);
        let mut proof = custom_cross_market_signal_proof();

        let err = proof
            .evaluate_acceptance_with_registry(&registry)
            .unwrap_err();

        assert!(
            err.to_string().contains("cross_market_components")
                && err.to_string().contains("reference_leg"),
            "{err}"
        );

        proof
            .cross_market_components
            .push(kimchi_component("reference_leg"));
        proof
            .evaluate_acceptance_with_registry(&registry)
            .expect("registry-declared cross-market roles should satisfy custom signal proof");
    }

    #[test]
    fn kimchi_premium_source_proof_rejects_future_leaking_reference_or_fx_components() {
        let registry = kimchi_registry();
        let mut proof = kimchi_signal_proof();
        proof
            .cross_market_components
            .iter_mut()
            .find(|component| component.role == "reference_price")
            .expect("reference component")
            .available_at_utc = "2026-03-01T00:00:01Z".to_string();

        let err = proof
            .evaluate_acceptance_with_registry(&registry)
            .unwrap_err();

        assert!(
            err.to_string().contains("available_at_utc") && err.to_string().contains("future"),
            "{err}"
        );
    }

    #[test]
    fn all_checks_passed_when_every_check_passes() {
        assert!(passing_checks().all_acceptable());
        assert!(passing_checks().unmet().is_empty());
    }

    #[test]
    fn pending_check_is_reported_as_unmet() {
        let mut checks = passing_checks();
        checks.license = RequiredCheck::pending("manual review outstanding");
        assert!(!checks.all_acceptable());
        assert_eq!(checks.unmet(), vec!["license"]);
    }

    #[test]
    fn source_proof_schema_requires_candidate_selection_cost_and_l2_evidence_fields() {
        let proof_json = serde_json::to_value(candidate_proof()).expect("serialize proof");
        for field in [
            "source_candidate_class",
            "source_selection_status",
            "cost_ref",
            "l2_replay_evidence",
        ] {
            let mut missing = proof_json.clone();
            missing
                .as_object_mut()
                .expect("source proof is an object")
                .remove(field);

            let err = serde_json::from_value::<SourceProofReport>(missing)
                .expect_err("missing schema field must not deserialize");

            assert!(
                err.to_string().contains(field),
                "missing field {field:?} should be named in error: {err}"
            );
        }

        for check in ["retention_freshness", "cost"] {
            let mut missing = proof_json.clone();
            missing
                .get_mut("required_checks")
                .and_then(serde_json::Value::as_object_mut)
                .expect("required_checks is an object")
                .remove(check);

            let err = serde_json::from_value::<SourceProofReport>(missing)
                .expect_err("missing required check must not deserialize");

            assert!(
                err.to_string().contains(check),
                "missing check {check:?} should be named in error: {err}"
            );
        }
    }

    #[test]
    fn paid_vendor_candidates_require_recorded_official_free_gap() {
        let mut proof = candidate_proof();
        proof.source_candidate_class = SourceCandidateClass::PaidVendor;
        proof.official_free_gap_ref = None;

        let err = proof.evaluate_acceptance().unwrap_err();

        assert_eq!(err, AcceptanceError::MissingField("official_free_gap_ref"));
    }

    #[test]
    fn non_selected_candidates_cannot_be_accepted_for_backfill_input() {
        let mut proof = candidate_proof();
        proof.source_selection_status = SourceSelectionStatus::PendingMoreProof;

        let err = proof.evaluate_acceptance().unwrap_err();

        assert_eq!(
            err,
            AcceptanceError::SourceSelectionNotAccepted(SourceSelectionStatus::PendingMoreProof)
        );
    }

    #[test]
    fn one_off_backfill_data_cannot_be_accepted_as_canonical_source_proof() {
        let mut proof = candidate_proof();
        proof.usage_scope = SourceProofUsageScope::OneOffBackfillData;

        let err = proof.evaluate_acceptance().unwrap_err();

        assert_eq!(err, AcceptanceError::OneOffBackfillDataNotCanonical);
    }

    #[test]
    fn forward_capture_candidates_require_paid_vendor_gap_evidence() {
        let mut proof = candidate_proof();
        proof.source_candidate_class = SourceCandidateClass::ForwardCapture;
        proof.official_free_gap_ref = Some("source-proof-gap://official-free-l2".to_string());
        proof.paid_vendor_gap_ref = None;

        let err = proof.evaluate_acceptance().unwrap_err();

        assert_eq!(err, AcceptanceError::MissingField("paid_vendor_gap_ref"));
    }

    #[test]
    fn l2_replay_requires_order_book_delta_or_sufficient_snapshot_cadence_evidence() {
        let mut proof = candidate_proof();
        proof.fidelity_class = SourceProofFidelityClass::L2Replay;
        proof.forbidden_claims.clear();
        proof.claim_limits.clear();
        proof.l2_replay_evidence = L2ReplayEvidence {
            order_book_delta_ref: None,
            sufficient_snapshot_cadence_ref: None,
            no_tick_size_change_universe_ref: None,
            timed_instrument_epoch_replay_ref: None,
        };

        let err = proof.evaluate_acceptance().unwrap_err();

        assert_eq!(err, AcceptanceError::MissingField("l2_replay_evidence"));
    }

    #[test]
    fn l2_replay_requires_tick_size_policy_evidence() {
        let mut proof = candidate_proof();
        proof.fidelity_class = SourceProofFidelityClass::L2Replay;
        proof.forbidden_claims.clear();
        proof.claim_limits.clear();
        proof.l2_replay_evidence = L2ReplayEvidence {
            order_book_delta_ref: Some("source-proof://order-book-deltas".to_string()),
            sufficient_snapshot_cadence_ref: None,
            no_tick_size_change_universe_ref: None,
            timed_instrument_epoch_replay_ref: None,
        };

        let err = proof.evaluate_acceptance().unwrap_err();

        assert_eq!(
            err,
            AcceptanceError::MissingField("l2_replay_evidence.tick_size_policy")
        );
    }

    #[test]
    fn l2_replay_accepts_source_bound_no_tick_size_change_policy() {
        let mut proof = candidate_proof();
        proof.fidelity_class = SourceProofFidelityClass::L2Replay;
        proof.forbidden_claims.clear();
        proof.claim_limits.clear();
        proof.l2_replay_evidence = L2ReplayEvidence {
            order_book_delta_ref: Some("source-proof://order-book-deltas".to_string()),
            sufficient_snapshot_cadence_ref: None,
            no_tick_size_change_universe_ref: Some(
                "source-proof://no-tick-size-change-universe".to_string(),
            ),
            timed_instrument_epoch_replay_ref: None,
        };

        proof.evaluate_acceptance().unwrap();
    }

    #[test]
    fn l2_replay_accepts_timed_instrument_epoch_replay_policy() {
        let mut proof = candidate_proof();
        proof.fidelity_class = SourceProofFidelityClass::L2Replay;
        proof.forbidden_claims.clear();
        proof.claim_limits.clear();
        proof.l2_replay_evidence = L2ReplayEvidence {
            order_book_delta_ref: Some("source-proof://order-book-deltas".to_string()),
            sufficient_snapshot_cadence_ref: None,
            no_tick_size_change_universe_ref: None,
            timed_instrument_epoch_replay_ref: Some(
                "source-proof://timed-instrument-epoch-replay".to_string(),
            ),
        };

        proof.evaluate_acceptance().unwrap();
    }

    #[test]
    fn pending_reports_must_not_carry_acceptance_provenance() {
        let mut proof = candidate_proof();
        proof.acceptance_mode = Some(AcceptanceMode::Manual);

        let err = proof.evaluate_acceptance().unwrap_err();

        assert_eq!(
            err,
            AcceptanceError::UnexpectedField {
                field: "acceptance_mode",
                reason: "acceptance provenance is only valid on accepted reports",
            }
        );
    }

    #[test]
    fn check_with_empty_evidence_is_unmet() {
        let mut checks = passing_checks();
        checks.nt_mapping = RequiredCheck::passed("");
        assert_eq!(checks.unmet(), vec!["nt_mapping"]);
    }

    #[test]
    fn not_applicable_required_check_requires_matching_claim_limit_evidence() {
        let mut proof = candidate_proof();
        let evidence_ref = "source-proof://claim-limit/instrument-universe-not-historical";
        proof.required_checks.instrument_universe = RequiredCheck {
            outcome: CheckOutcome::NotApplicable,
            evidence_ref: evidence_ref.to_string(),
            expires_at_utc: None,
        };

        let err = proof.evaluate_acceptance().unwrap_err();
        assert!(
            err.to_string()
                .contains("required_checks.instrument_universe")
                && err.to_string().contains("claim_limits.evidence_ref"),
            "{err}"
        );

        proof.claim_limits.push(SourceProofClaimLimit {
            id: "instrument-universe-current-only".to_string(),
            severity: "blocking".to_string(),
            claim: "No historical venue-rule, fillability, rounding, sizing, or execution-quality claims.".to_string(),
            reason: "instrument metadata is a current construction snapshot, not a historical universe snapshot.".to_string(),
            evidence_ref: evidence_ref.to_string(),
        });

        proof.evaluate_acceptance().expect(
            "not_applicable check is acceptable only when a claim limit binds the same evidence",
        );
    }

    #[test]
    fn candidate_with_all_checks_can_be_accepted() {
        let accepted = candidate_proof()
            .accept(
                AcceptanceMode::Manual,
                "vertical-slice-operator",
                "2026-06-02T00:00:00Z",
            )
            .expect("candidate with passing checks should accept");
        assert_eq!(accepted.status, SourceProofStatus::Accepted);
        assert_eq!(accepted.acceptance_mode, Some(AcceptanceMode::Manual));
        assert!(accepted.is_accepted());
    }

    #[test]
    fn accepted_dataset_carries_acceptance_provenance() {
        let object = manifest_object();
        let proof = candidate_proof()
            .accept(
                AcceptanceMode::Manual,
                "vertical-slice-operator",
                "2026-06-02T00:00:00Z",
            )
            .expect("accepted proof");

        let accepted =
            select_accepted_dataset(&proof, &object, &object.sha256).expect("accepted dataset");

        assert_eq!(accepted.acceptance_mode, AcceptanceMode::Manual);
        assert_eq!(accepted.accepted_by, "vertical-slice-operator");
        assert_eq!(accepted.accepted_at, "2026-06-02T00:00:00Z");
        assert_eq!(accepted.accepted_object_sha256, object.sha256);
    }

    #[test]
    fn accept_rejects_non_pending_proof() {
        // A rejected proof must not be silently re-promoted to accepted.
        let mut proof = candidate_proof();
        proof.status = SourceProofStatus::Rejected;
        let err = proof
            .accept(AcceptanceMode::Manual, "operator", "2026-06-02T00:00:00Z")
            .unwrap_err();
        assert_eq!(
            err,
            AcceptanceError::NotPending(SourceProofStatus::Rejected)
        );
    }

    #[test]
    fn accept_rejects_already_accepted_proof() {
        let mut proof = candidate_proof();
        proof.status = SourceProofStatus::Accepted;
        let err = proof
            .accept(AcceptanceMode::Manual, "operator", "2026-06-02T00:00:00Z")
            .unwrap_err();
        assert_eq!(
            err,
            AcceptanceError::NotPending(SourceProofStatus::Accepted)
        );
    }

    #[test]
    fn accept_rejects_blank_accepted_by() {
        let err = candidate_proof()
            .accept(AcceptanceMode::Manual, "  ", "2026-06-02T00:00:00Z")
            .unwrap_err();
        assert_eq!(err, AcceptanceError::MissingField("accepted_by"));
    }

    #[test]
    fn accept_rejects_blank_accepted_at() {
        let err = candidate_proof()
            .accept(AcceptanceMode::Manual, "operator", "  ")
            .unwrap_err();
        assert_eq!(err, AcceptanceError::MissingField("accepted_at"));
    }

    #[test]
    fn acceptance_blocked_when_supersedes_is_self_referential() {
        let mut proof = candidate_proof();
        proof.supersedes_source_proof_id = Some(proof.source_proof_id.clone());
        assert_eq!(
            proof.evaluate_acceptance().unwrap_err(),
            AcceptanceError::SelfReferentialSupersede
        );
    }

    #[test]
    fn acceptance_blocked_when_supersedes_is_blank() {
        let mut proof = candidate_proof();
        proof.supersedes_source_proof_id = Some("  ".to_string());
        assert_eq!(
            proof.evaluate_acceptance().unwrap_err(),
            AcceptanceError::MissingField("supersedes_source_proof_id")
        );
    }

    #[test]
    fn select_rejects_object_from_other_venue() {
        // An object whose provenance URL names a different venue than the proof
        // must not be admitted under that proof.
        let accepted = candidate_proof()
            .accept(AcceptanceMode::Manual, "operator", "2026-06-02T00:00:00Z")
            .unwrap();
        let mut object = manifest_object();
        object.source_url =
            "https://public.okx.com/spot/BNBUSDC/BNBUSDC_2026-03-01.csv.gz".to_string();
        let err = select_accepted_dataset(&accepted, &object, &object.sha256).unwrap_err();
        assert!(
            matches!(err, AcceptanceError::SourceVenueMismatch { .. }),
            "{err:?}"
        );
    }

    #[test]
    fn select_rejects_substring_venue_host_match() {
        let accepted = candidate_proof()
            .accept(AcceptanceMode::Manual, "operator", "2026-06-02T00:00:00Z")
            .unwrap();
        let mut object = manifest_object();
        object.source_url =
            "https://evil-bybit-mirror.example/spot/BNBUSDC/BNBUSDC_2026-03-01.csv.gz".to_string();
        let err = select_accepted_dataset(&accepted, &object, &object.sha256).unwrap_err();
        assert!(
            matches!(err, AcceptanceError::SourceVenueMismatch { .. }),
            "{err:?}"
        );
    }

    #[test]
    fn select_rejects_venue_label_on_untrusted_domain() {
        let accepted = candidate_proof()
            .accept(AcceptanceMode::Manual, "operator", "2026-06-02T00:00:00Z")
            .unwrap();
        let mut object = manifest_object();
        object.source_url =
            "https://bybit.evil.example/spot/BNBUSDC/BNBUSDC_2026-03-01.csv.gz".to_string();
        let err = select_accepted_dataset(&accepted, &object, &object.sha256).unwrap_err();
        assert!(
            matches!(err, AcceptanceError::SourceVenueMismatch { .. }),
            "{err:?}"
        );
    }

    #[test]
    fn select_rejects_unconfigured_single_label_tld_host() {
        let accepted = candidate_proof()
            .accept(AcceptanceMode::Manual, "operator", "2026-06-02T00:00:00Z")
            .unwrap();
        let mut object = manifest_object();
        object.source_url = "https://bybit.evil/spot/BNBUSDC/BNBUSDC_2026-03-01.csv.gz".to_string();
        let err = select_accepted_dataset(&accepted, &object, &object.sha256).unwrap_err();
        assert!(
            matches!(err, AcceptanceError::SourceVenueMismatch { .. }),
            "{err:?}"
        );
    }

    #[test]
    fn select_rejects_unconfigured_fake_tld_subdomain() {
        let accepted = candidate_proof()
            .accept(AcceptanceMode::Manual, "operator", "2026-06-02T00:00:00Z")
            .unwrap();
        let mut object = manifest_object();
        object.source_url =
            "https://evil.bybit.fake/spot/BNBUSDC/BNBUSDC_2026-03-01.csv.gz".to_string();
        let err = select_accepted_dataset(&accepted, &object, &object.sha256).unwrap_err();
        assert!(
            matches!(err, AcceptanceError::SourceVenueMismatch { .. }),
            "{err:?}"
        );
    }

    #[test]
    fn select_rejects_unconfigured_non_com_source_host() {
        let accepted = candidate_proof()
            .accept(AcceptanceMode::Manual, "operator", "2026-06-02T00:00:00Z")
            .unwrap();
        let mut object = manifest_object();
        object.source_url =
            "https://data.bybit.net/spot/BNBUSDC/BNBUSDC_2026-03-01.csv.gz".to_string();
        let err = select_accepted_dataset(&accepted, &object, &object.sha256).unwrap_err();
        assert!(
            matches!(err, AcceptanceError::SourceVenueMismatch { .. }),
            "{err:?}"
        );
    }

    #[test]
    fn select_rejects_unconfigured_multilabel_public_suffix_host() {
        let accepted = candidate_proof()
            .accept(AcceptanceMode::Manual, "operator", "2026-06-02T00:00:00Z")
            .unwrap();
        let mut object = manifest_object();
        object.source_url =
            "https://data.bybit.co.uk/spot/BNBUSDC/BNBUSDC_2026-03-01.csv.gz".to_string();
        let err = select_accepted_dataset(&accepted, &object, &object.sha256).unwrap_err();
        assert!(
            matches!(err, AcceptanceError::SourceVenueMismatch { .. }),
            "{err:?}"
        );
    }

    #[test]
    fn acceptance_requires_source_binding_market_structure_fixture() {
        let err = candidate_proof()
            .accept_with_registry(
                &source_binding_registry_without_market_structure_fixture(),
                AcceptanceMode::Manual,
                "operator",
                "2026-06-02T00:00:00Z",
            )
            .unwrap_err();

        assert_eq!(
            err,
            AcceptanceError::MissingField("source_binding.market_structure_fixture")
        );
    }

    #[test]
    fn acceptance_rejects_source_binding_market_structure_fixture_mismatch() {
        let err = candidate_proof()
            .accept_with_registry(
                &source_binding_registry_with_market_structure_fixture("binary-option"),
                AcceptanceMode::Manual,
                "operator",
                "2026-06-02T00:00:00Z",
            )
            .unwrap_err();

        assert!(matches!(
            err,
            AcceptanceError::SourceBindingMismatch {
                field: "market_structure_fixture",
                ..
            }
        ));
    }

    #[test]
    fn acceptance_rejects_legacy_non_bte_market_structure_fixture() {
        let mut proof = candidate_proof();
        proof.fixture_type = FixtureType::PredictionMarket;

        let err = proof
            .accept_with_registry(
                &source_binding_registry_with_market_structure_fixture("prediction-market"),
                AcceptanceMode::Manual,
                "operator",
                "2026-06-02T00:00:00Z",
            )
            .unwrap_err();

        assert!(matches!(
            err,
            AcceptanceError::SourceBindingMismatch {
                field: "market_structure_fixture",
                ..
            }
        ));
    }

    #[test]
    fn committed_registry_exposes_required_market_structure_fixtures() {
        let registry = committed_source_binding_registry();

        for (source_binding, product_family) in [
            ("bybit-spot-tick-trades", "spot"),
            ("bybit-linear-tick-trades", "linear"),
            ("bybit-inverse-tick-trades", "inverse"),
        ] {
            let bybit_trade = registry
                .source_binding_metadata(source_binding, "bybit")
                .expect("Bybit public archive tick-trades binding");
            assert_eq!(bybit_trade.product_family, product_family);
            assert_eq!(
                bybit_trade.market_structure_fixture,
                Some(FixtureType::PerpsSpot)
            );
            assert_eq!(
                bybit_trade.evidence_state,
                EvidenceState::OwnerArchiveBackfillable
            );
            assert!(bybit_trade.table_families.contains(&"trades".to_string()));
        }

        for (source_binding, product_family) in [
            ("binance-usd-m-perpetual-native-trades", "usd_m_perpetual"),
            ("binance-usd-m-delivery-native-trades", "usd_m_delivery"),
            ("binance-coin-m-perpetual-native-trades", "coin_m_perpetual"),
            ("binance-coin-m-delivery-native-trades", "coin_m_delivery"),
        ] {
            let binance_trade = registry
                .source_binding_metadata(source_binding, "binance")
                .expect("Binance Data Vision futures native-trades binding");
            assert_eq!(binance_trade.product_family, product_family);
            assert_eq!(
                binance_trade.market_structure_fixture,
                Some(FixtureType::PerpsSpot)
            );
            assert_eq!(
                binance_trade.evidence_state,
                EvidenceState::DirectlyBackfillable
            );
            assert!(binance_trade.table_families.contains(&"trades".to_string()));
        }

        let binary_option = registry
            .source_binding_metadata("hyperliquid-hip4-outcome-meta", "hyperliquid")
            .expect("binary-option sample binding");
        assert_eq!(
            binary_option.market_structure_fixture,
            Some(FixtureType::BinaryOption)
        );

        let kalshi = registry
            .source_binding_metadata("kalshi-official-historical-api", "kalshi")
            .expect("Kalshi historical candidate binding");
        assert_eq!(
            kalshi.market_structure_fixture,
            Some(FixtureType::BinaryOption)
        );
        assert_eq!(kalshi.evidence_state, EvidenceState::PendingSourceProof);
        assert!(kalshi.table_families.contains(&"bars".to_string()));
        assert!(kalshi.table_families.contains(&"trades".to_string()));
        assert!(
            !kalshi
                .table_families
                .contains(&"order_book_deltas".to_string()),
            "official Kalshi historical API must not claim historical order-book deltas"
        );
    }

    #[test]
    fn select_rejects_unknown_source_binding() {
        let mut accepted = candidate_proof();
        accepted.status = SourceProofStatus::Accepted;
        accepted.acceptance_mode = Some(AcceptanceMode::Manual);
        accepted.accepted_by = Some("operator".to_string());
        accepted.accepted_at = Some("2026-06-02T00:00:00Z".to_string());
        accepted.source_binding = "bybit-does-not-exist".to_string();
        let object = manifest_object();
        let err = select_accepted_dataset(&accepted, &object, &object.sha256).unwrap_err();
        assert!(
            matches!(err, AcceptanceError::UnknownSourceBinding { .. }),
            "{err:?}"
        );
    }

    #[test]
    fn select_rejects_non_https_source_url() {
        let accepted = candidate_proof()
            .accept(AcceptanceMode::Manual, "operator", "2026-06-02T00:00:00Z")
            .unwrap();
        let mut object = manifest_object();
        object.source_url =
            "ftp://public.bybit.com/spot/BNBUSDC/BNBUSDC_2026-03-01.csv.gz".to_string();
        let err = select_accepted_dataset(&accepted, &object, &object.sha256).unwrap_err();
        assert!(
            matches!(err, AcceptanceError::SourceVenueMismatch { .. }),
            "{err:?}"
        );
    }

    #[test]
    fn select_rejects_schemeless_source_url() {
        let accepted = candidate_proof()
            .accept(AcceptanceMode::Manual, "operator", "2026-06-02T00:00:00Z")
            .unwrap();
        let mut object = manifest_object();
        object.source_url = "public.bybit.com/spot/BNBUSDC/BNBUSDC_2026-03-01.csv.gz".to_string();
        let err = select_accepted_dataset(&accepted, &object, &object.sha256).unwrap_err();
        assert!(
            matches!(err, AcceptanceError::SourceVenueMismatch { .. }),
            "{err:?}"
        );
    }

    #[test]
    fn select_rejects_extra_label_before_configured_host() {
        let accepted = candidate_proof()
            .accept(AcceptanceMode::Manual, "operator", "2026-06-02T00:00:00Z")
            .unwrap();
        let mut object = manifest_object();
        object.source_url =
            "https://evil.public.bybit.com/spot/BNBUSDC/BNBUSDC_2026-03-01.csv.gz".to_string();
        let err = select_accepted_dataset(&accepted, &object, &object.sha256).unwrap_err();
        assert!(
            matches!(err, AcceptanceError::SourceVenueMismatch { .. }),
            "{err:?}"
        );
    }

    #[test]
    fn select_rejects_same_host_path_outside_declared_source_template() {
        let mut proof = candidate_proof();
        proof.source_proof_id = "source-proof-binance-spot-native-trades".to_string();
        proof.source_binding = "binance-spot-native-trades".to_string();
        proof.venue = "binance".to_string();
        proof.evidence_state = EvidenceState::DirectlyBackfillable;
        proof.raw_sample_uri =
            "s3://bolt-parquet/.../binance/raw/object=monthly-trades.zip".to_string();
        let raw_sample_uri = proof.raw_sample_uri.clone();
        let accepted = proof
            .accept(AcceptanceMode::Manual, "operator", "2026-06-02T00:00:00Z")
            .unwrap();
        let mut object = manifest_object();
        object.s3_uri = raw_sample_uri;
        object.source_url =
            "https://data.binance.vision/data/spot/monthly/trades/BNBUSDC/BNBUSDC-trades-2026-03.zip"
                .to_string();

        let err = select_accepted_dataset(&accepted, &object, &object.sha256).unwrap_err();

        assert!(
            matches!(err, AcceptanceError::SourceVenueMismatch { .. }),
            "{err:?}"
        );
    }

    #[test]
    fn select_accepts_configured_source_host_with_url_variations() {
        let accepted = candidate_proof()
            .accept(AcceptanceMode::Manual, "operator", "2026-06-02T00:00:00Z")
            .unwrap();
        let mut object = manifest_object();
        object.source_url =
            "https://operator@PUBLIC.BYBIT.COM.:443/spot/BNBUSDC/BNBUSDC_2026-03-01.csv.gz"
                .to_string();
        select_accepted_dataset(&accepted, &object, &object.sha256).unwrap();
    }

    #[test]
    fn select_rejects_malformed_coverage_bound() {
        let mut proof = candidate_proof();
        proof.coverage_time_range.end_utc = "not-a-timestamp".to_string();
        let err = proof
            .accept(AcceptanceMode::Manual, "operator", "2026-06-02T00:00:00Z")
            .unwrap_err();
        assert!(
            matches!(err, AcceptanceError::MalformedCoverageBound { .. }),
            "{err:?}"
        );
    }

    #[test]
    fn select_rejects_inverted_coverage_window() {
        let mut proof = candidate_proof();
        proof.coverage_time_range.start_utc = "2026-03-05T00:00:00Z".to_string();
        proof.coverage_time_range.end_utc = "2026-03-01T00:00:00Z".to_string();
        let err = proof
            .accept(AcceptanceMode::Manual, "operator", "2026-06-02T00:00:00Z")
            .unwrap_err();
        assert!(
            matches!(err, AcceptanceError::MalformedCoverageBound { .. }),
            "{err:?}"
        );
    }

    #[test]
    fn select_excludes_object_on_exclusive_end_bound() {
        // Coverage end is exclusive: an object dated exactly on end_utc is rejected.
        let mut proof = candidate_proof();
        proof.coverage_time_range.start_utc = "2026-03-01T00:00:00Z".to_string();
        proof.coverage_time_range.end_utc = "2026-03-02T00:00:00Z".to_string();
        let accepted = proof
            .accept(AcceptanceMode::Manual, "operator", "2026-06-02T00:00:00Z")
            .unwrap();
        let mut on_end = manifest_object();
        on_end.archive_date = "2026-03-02".to_string();
        let err = select_accepted_dataset(&accepted, &on_end, &on_end.sha256).unwrap_err();
        assert!(
            matches!(err, AcceptanceError::OutsideCoverage { .. }),
            "{err:?}"
        );
    }

    #[test]
    fn acceptance_blocked_when_any_check_pending() {
        let mut proof = candidate_proof();
        proof.required_checks.coverage = RequiredCheck::pending("coverage not proven");
        let err = proof.evaluate_acceptance().unwrap_err();
        assert_eq!(err, AcceptanceError::UnmetChecks(vec!["coverage"]));
    }

    #[test]
    fn acceptance_rejects_coverage_outside_requested_window() {
        let mut proof = candidate_proof();
        proof.coverage_time_range.start_utc = "2025-05-31T00:00:00Z".to_string();
        let err = proof
            .accept(AcceptanceMode::Manual, "operator", "2026-06-02T00:00:00Z")
            .unwrap_err();
        assert!(err.to_string().contains("requested"), "{err}");
    }

    #[test]
    fn evaluate_acceptance_rejects_rejected_status() {
        let mut proof = candidate_proof();
        proof.status = SourceProofStatus::Rejected;
        let err = proof.evaluate_acceptance().unwrap_err();
        assert!(err.to_string().contains("rejected"), "{err}");
    }

    #[test]
    fn acceptance_blocked_when_identity_field_missing() {
        let mut proof = candidate_proof();
        proof.license_ref = "  ".to_string();
        assert_eq!(
            proof.evaluate_acceptance().unwrap_err(),
            AcceptanceError::MissingField("license_ref")
        );
    }

    #[test]
    fn non_l2_fidelity_requires_forbidden_claims() {
        let mut proof = candidate_proof();
        proof.forbidden_claims.clear();
        assert_eq!(
            proof.evaluate_acceptance().unwrap_err(),
            AcceptanceError::ForbiddenClaimMissing
        );
    }

    #[test]
    fn non_l2_fidelity_requires_structured_claim_limits() {
        let mut proof = candidate_proof();
        proof.claim_limits.clear();
        let err = proof.evaluate_acceptance().unwrap_err();
        assert!(err.to_string().contains("claim_limits"), "{err}");
    }

    #[test]
    fn structured_claim_limits_must_cover_forbidden_claims() {
        let mut proof = candidate_proof();
        proof.claim_limits[0].claim = "No unrelated claim.".to_string();
        let err = proof.evaluate_acceptance().unwrap_err();
        assert!(err.to_string().contains("forbidden_claims"), "{err}");
    }

    #[test]
    fn l2_replay_forbidden_claims_require_structured_claim_limits() {
        let mut proof = candidate_proof();
        proof.fidelity_class = SourceProofFidelityClass::L2Replay;
        proof.l2_replay_evidence.order_book_delta_ref =
            Some("source-proof://l2-order-book-deltas".to_string());
        proof.l2_replay_evidence.no_tick_size_change_universe_ref =
            Some("source-proof://no-tick-size-change-universe".to_string());
        proof.forbidden_claims = vec!["No dynamic instrument-epoch replay claim.".to_string()];
        proof.claim_limits.clear();

        let err = proof.evaluate_acceptance().unwrap_err();

        assert!(err.to_string().contains("forbidden_claims"), "{err}");
    }

    #[test]
    fn acceptance_blocked_when_contract_version_unexpected() {
        let mut proof = candidate_proof();
        proof.contract_version = "some-other-contract.v9".to_string();
        assert_eq!(
            proof.evaluate_acceptance().unwrap_err(),
            AcceptanceError::UnexpectedVersion {
                field: "contract_version",
                expected: CONTRACT_VERSION,
                actual: "some-other-contract.v9".to_string(),
            }
        );
    }

    #[test]
    fn acceptance_blocked_when_schema_version_unexpected() {
        let mut proof = candidate_proof();
        proof.schema_version = "backfill-source-proof.v0".to_string();
        assert_eq!(
            proof.evaluate_acceptance().unwrap_err(),
            AcceptanceError::UnexpectedVersion {
                field: "schema_version",
                expected: SOURCE_PROOF_SCHEMA_VERSION,
                actual: "backfill-source-proof.v0".to_string(),
            }
        );
    }

    #[test]
    fn acceptance_blocked_when_nt_mapping_not_accepted() {
        let mut proof = candidate_proof();
        proof.nt_mapping_status = NtMappingStatus::Pending;
        assert_eq!(
            proof.evaluate_acceptance().unwrap_err(),
            AcceptanceError::NtMappingNotAccepted(NtMappingStatus::Pending)
        );
    }

    #[test]
    fn acceptance_blocked_when_evidence_state_is_not_backfillable() {
        for evidence_state in [
            EvidenceState::BoundedOrCurrentOnly,
            EvidenceState::PendingSourceProof,
            EvidenceState::VendorOrForwardCaptureOnly,
            EvidenceState::NotApplicable,
            EvidenceState::ExcludedFromCurrentScope,
        ] {
            let mut proof = candidate_proof();
            proof.evidence_state = evidence_state;

            let err = proof.evaluate_acceptance().unwrap_err();

            assert!(
                err.to_string().contains("evidence_state")
                    && err.to_string().contains("backfillable"),
                "{evidence_state:?}: {err}"
            );
        }
    }

    #[test]
    fn acceptance_blocked_when_structured_scope_summary_missing() {
        let mut proof = candidate_proof();
        proof.acceptance_scope = None;

        assert_eq!(
            proof.evaluate_acceptance().unwrap_err(),
            AcceptanceError::MissingField("acceptance_scope")
        );
    }

    #[test]
    fn acceptance_blocked_when_structured_scope_summary_has_failures_or_scope_violations() {
        let mut proof = candidate_proof();
        let scope = proof.acceptance_scope.as_mut().expect("acceptance scope");
        scope.failed_objects = 1;

        let err = proof.evaluate_acceptance().unwrap_err();

        assert!(
            err.to_string().contains("failed_objects") && err.to_string().contains("must be zero"),
            "{err}"
        );

        let mut proof = candidate_proof();
        let scope = proof.acceptance_scope.as_mut().expect("acceptance scope");
        scope.selector_scope_violations = 1;

        let err = proof.evaluate_acceptance().unwrap_err();

        assert!(
            err.to_string().contains("selector_scope_violations")
                && err.to_string().contains("must be zero"),
            "{err}"
        );
    }

    #[test]
    fn acceptance_blocked_when_source_binding_family_disagrees_with_registry() {
        let mut proof = candidate_proof();
        proof.product_family = "linear".to_string();

        let err = proof.evaluate_acceptance().unwrap_err();

        assert!(
            err.to_string().contains("source_binding")
                && err.to_string().contains("product_family")
                && err.to_string().contains("spot")
                && err.to_string().contains("linear"),
            "{err}"
        );

        let mut proof = candidate_proof();
        proof.table_family = "order_book_snapshots_fixed_depth".to_string();

        let err = proof.evaluate_acceptance().unwrap_err();

        assert!(
            err.to_string().contains("source_binding")
                && err.to_string().contains("table_family")
                && err.to_string().contains("trades")
                && err.to_string().contains("order_book_snapshots_fixed_depth"),
            "{err}"
        );

        let mut proof = candidate_proof();
        proof.evidence_state = EvidenceState::DirectlyBackfillable;

        let err = proof.evaluate_acceptance().unwrap_err();

        assert!(
            err.to_string().contains("source_binding")
                && err.to_string().contains("evidence_state")
                && err.to_string().contains("OwnerArchiveBackfillable")
                && err.to_string().contains("DirectlyBackfillable"),
            "{err}"
        );
    }

    #[test]
    fn acceptance_blocked_when_source_binding_missing_from_registry() {
        let mut proof = candidate_proof();
        proof.source_binding = "missing-native-trades".to_string();

        let err = proof.evaluate_acceptance().unwrap_err();

        assert!(
            err.to_string().contains("source_binding")
                && err.to_string().contains("registry")
                && err.to_string().contains("missing-native-trades"),
            "{err}"
        );
    }

    #[test]
    fn native_trade_source_bindings_cover_multiple_configured_venues() {
        let registry: toml::Value =
            toml::from_str(SOURCE_BINDINGS_REGISTRY).expect("source bindings registry parses");
        let bindings = registry
            .get("source_binding")
            .and_then(toml::Value::as_array)
            .expect("source_binding array");
        let mut venues = std::collections::BTreeSet::new();
        let mut keys = Vec::new();
        for binding in bindings {
            let table_families = binding
                .get("table_families")
                .and_then(toml::Value::as_array)
                .expect("table_families array");
            let is_native_trade_fixture =
                binding.get("fixture").and_then(toml::Value::as_str) == Some("native-trades");
            let is_trade_table = table_families
                .iter()
                .any(|family| family.as_str() == Some("trades"));
            let is_backfillable = matches!(
                binding.get("evidence_state").and_then(toml::Value::as_str),
                Some("directly_backfillable" | "owner_archive_backfillable")
            );
            if is_native_trade_fixture && is_trade_table && is_backfillable {
                let key = binding
                    .get("key")
                    .and_then(toml::Value::as_str)
                    .expect("key");
                let venue = binding
                    .get("venue")
                    .and_then(toml::Value::as_str)
                    .expect("venue");
                let product_family = binding
                    .get("product_family")
                    .and_then(toml::Value::as_str)
                    .expect("product_family");
                let evidence_state = match binding
                    .get("evidence_state")
                    .and_then(toml::Value::as_str)
                    .expect("evidence_state")
                {
                    "directly_backfillable" => EvidenceState::DirectlyBackfillable,
                    "owner_archive_backfillable" => EvidenceState::OwnerArchiveBackfillable,
                    other => panic!("unexpected backfillable evidence state {other:?}"),
                };
                let source_uri = binding
                    .get("source_uri")
                    .and_then(toml::Value::as_str)
                    .expect("source_uri");

                let mut proof = candidate_proof();
                proof.source_proof_id = format!("source-proof-{key}");
                proof.source_binding = key.to_string();
                proof.venue = venue.to_string();
                proof.product_family = product_family.to_string();
                proof.product_category = product_family.to_string();
                proof.evidence_state = evidence_state;
                proof
                    .evaluate_acceptance()
                    .expect("configured native-trades source binding should pass proof acceptance");
                let proof = proof
                    .accept(AcceptanceMode::Manual, "operator", "2026-06-02T00:00:00Z")
                    .expect("configured native-trades source binding should be acceptable");

                let mut object = manifest_object();
                object.source_url = source_uri
                    .replace("{symbol}", "BNBUSDC")
                    .replace("{dt}", "2026-03-01");
                select_accepted_dataset(&proof, &object, &object.sha256).expect(
                    "configured native-trades source binding should select by source template",
                );

                venues.insert(venue.to_string());
                keys.push(key.to_string());
            }
        }

        assert!(
            venues.len() >= 2,
            "native trade bindings must cover at least two configured venues; found venues={venues:?}, keys={keys:?}"
        );
    }

    #[test]
    fn acceptance_blocked_when_schema_sample_hash_missing() {
        let mut proof = candidate_proof();
        proof.schema_sample_hash = "  ".to_string();
        assert_eq!(
            proof.evaluate_acceptance().unwrap_err(),
            AcceptanceError::MissingField("schema_sample_hash")
        );
    }

    #[test]
    fn acceptance_blocked_when_raw_sample_uri_is_not_staged_to_s3() {
        let mut proof = candidate_proof();
        proof.raw_sample_uri =
            "https://public.bybit.com/spot/BNBUSDC/BNBUSDC_2026-03-01.csv.gz".to_string();

        let err = proof.evaluate_acceptance().unwrap_err();

        assert!(
            err.to_string().contains("raw_sample_uri") && err.to_string().contains("s3://"),
            "{err}"
        );
    }

    #[test]
    fn acceptance_blocked_when_schema_sample_uri_is_not_staged_to_s3() {
        let mut proof = candidate_proof();
        proof.schema_sample_uri = "https://public.bybit.com/schema-sample.json".to_string();

        let err = proof.evaluate_acceptance().unwrap_err();

        assert!(
            err.to_string().contains("schema_sample_uri") && err.to_string().contains("s3://"),
            "{err}"
        );
    }

    #[test]
    fn acceptance_blocked_when_retention_ref_missing() {
        let mut proof = candidate_proof();
        proof.retention_ref = String::new();
        assert_eq!(
            proof.evaluate_acceptance().unwrap_err(),
            AcceptanceError::MissingField("retention_ref")
        );
    }

    #[test]
    fn ledger_admits_hash_verified_in_coverage_object() {
        let proof = candidate_proof()
            .accept(AcceptanceMode::Manual, "operator", "2026-06-02T00:00:00Z")
            .unwrap();
        let object = manifest_object();
        let dataset = select_accepted_dataset(&proof, &object, &object.sha256)
            .expect("hash-verified in-coverage object should be admitted");
        assert_eq!(dataset.source_proof_id, proof.source_proof_id);
        assert_eq!(
            dataset.fidelity_class,
            SourceProofFidelityClass::TradeReplay
        );
        assert_eq!(dataset.object.sha256, object.sha256);
    }

    #[test]
    fn ledger_rejects_when_proof_not_accepted() {
        let proof = candidate_proof(); // still pending
        let object = manifest_object();
        assert_eq!(
            select_accepted_dataset(&proof, &object, &object.sha256).unwrap_err(),
            AcceptanceError::ProofNotAccepted(SourceProofStatus::Pending)
        );
    }

    #[test]
    fn ledger_rejects_hash_mismatch() {
        let proof = candidate_proof()
            .accept(AcceptanceMode::Manual, "operator", "2026-06-02T00:00:00Z")
            .unwrap();
        let object = manifest_object();
        let err = select_accepted_dataset(&proof, &object, "deadbeef").unwrap_err();
        assert!(matches!(err, AcceptanceError::ContentHashMismatch { .. }));
    }

    #[test]
    fn ledger_rejects_object_hash_that_differs_from_proof_sample_hash() {
        let mut proof = candidate_proof();
        proof.raw_sample_hash =
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string();
        let proof = proof
            .accept(AcceptanceMode::Manual, "operator", "2026-06-02T00:00:00Z")
            .unwrap();
        let object = manifest_object();
        let err = select_accepted_dataset(&proof, &object, &object.sha256).unwrap_err();
        assert!(matches!(err, AcceptanceError::ContentHashMismatch { .. }));
    }

    #[test]
    fn ledger_rejects_manifest_object_from_different_staged_uri_than_raw_sample() {
        let proof = candidate_proof()
            .accept(AcceptanceMode::Manual, "operator", "2026-06-02T00:00:00Z")
            .unwrap();
        let mut object = manifest_object();
        object.s3_uri =
            "s3://bolt-parquet/.../symbol=BNBUSDC/object=different-object.csv.gz".to_string();

        let err = select_accepted_dataset(&proof, &object, &object.sha256).unwrap_err();

        assert!(
            err.to_string().contains("raw_sample_uri") && err.to_string().contains("s3_uri"),
            "{err}"
        );
    }

    #[test]
    fn ledger_rejects_raw_sample_that_was_not_staged_to_s3() {
        let mut proof = candidate_proof();
        proof.raw_sample_uri =
            "https://public.bybit.com/spot/BNBUSDC/BNBUSDC_2026-03-01.csv.gz".to_string();
        proof.status = SourceProofStatus::Accepted;
        proof.acceptance_mode = Some(AcceptanceMode::Manual);
        proof.accepted_by = Some("operator".to_string());
        proof.accepted_at = Some("2026-06-02T00:00:00Z".to_string());
        let mut object = manifest_object();
        object.s3_uri = proof.raw_sample_uri.clone();

        let err = select_accepted_dataset(&proof, &object, &object.sha256).unwrap_err();

        assert!(
            err.to_string().contains("raw_sample_uri") && err.to_string().contains("s3://"),
            "{err}"
        );
    }

    #[test]
    fn ledger_rejects_object_bytes_exceeding_structured_acceptance_scope() {
        let mut proof = candidate_proof();
        proof
            .acceptance_scope
            .as_mut()
            .expect("acceptance scope")
            .accepted_bytes = manifest_object().bytes - 1;
        let proof = proof
            .accept(AcceptanceMode::Manual, "operator", "2026-06-02T00:00:00Z")
            .unwrap();
        let object = manifest_object();

        let err = select_accepted_dataset(&proof, &object, &object.sha256).unwrap_err();

        assert!(
            err.to_string().contains("accepted_bytes")
                && err.to_string().contains("selected object bytes"),
            "{err}"
        );
    }

    #[test]
    fn ledger_rejects_missing_schema_columns() {
        let proof = candidate_proof()
            .accept(AcceptanceMode::Manual, "operator", "2026-06-02T00:00:00Z")
            .unwrap();
        let mut object = manifest_object();
        object.schema_columns.clear();
        assert_eq!(
            select_accepted_dataset(&proof, &object, &object.sha256).unwrap_err(),
            AcceptanceError::ManifestRecordIncomplete("schema_columns")
        );
    }

    #[test]
    fn ledger_rejects_object_outside_coverage() {
        let proof = candidate_proof()
            .accept(AcceptanceMode::Manual, "operator", "2026-06-02T00:00:00Z")
            .unwrap();
        let mut object = manifest_object();
        object.archive_date = "2026-04-01".to_string();
        let err = select_accepted_dataset(&proof, &object, &object.sha256).unwrap_err();
        assert!(matches!(err, AcceptanceError::OutsideCoverage { .. }));
    }

    #[test]
    fn accepted_proof_serializes_with_provenance() {
        let proof = candidate_proof()
            .accept(AcceptanceMode::Manual, "operator", "2026-06-02T00:00:00Z")
            .unwrap();
        let json = serde_json::to_string(&proof).expect("serialize");
        assert!(json.contains("\"status\":\"accepted\""));
        assert!(json.contains("\"fidelity_class\":\"TRADE_REPLAY\""));
        let round_trip: SourceProofReport = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(round_trip, proof);
    }
}
