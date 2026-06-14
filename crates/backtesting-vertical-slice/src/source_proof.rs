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
struct SourceBindingRegistry {
    #[serde(rename = "source_binding", default)]
    source_bindings: Vec<SourceBindingConfig>,
}

#[derive(Debug, Deserialize)]
struct SourceBindingConfig {
    key: String,
    venue: String,
    source_uri: String,
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

/// Market-structure fixture family the proof belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FixtureType {
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
/// queue-position, and order-book-liquidity claims.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SourceProofFidelityClass {
    L2Replay,
    SnapshotReplay,
    TradeReplay,
    TradeBarReplay,
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
/// Only [`Self::Passed`] contributes to acceptance. Any `Failed` or `Pending`
/// check keeps the proof out of canonical backfill/catalog/backtest selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckOutcome {
    Passed,
    Failed,
    Pending,
}

impl CheckOutcome {
    #[must_use]
    pub const fn is_passed(self) -> bool {
        matches!(self, Self::Passed)
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

/// A single required check result with its supporting evidence pointer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequiredCheck {
    pub outcome: CheckOutcome,
    /// Pointer to the evidence backing this check (URI, hash, manifest id, or
    /// recorded attestation). Required for an accepted proof.
    pub evidence_ref: String,
}

impl RequiredCheck {
    #[must_use]
    pub fn passed(evidence_ref: impl Into<String>) -> Self {
        Self {
            outcome: CheckOutcome::Passed,
            evidence_ref: evidence_ref.into(),
        }
    }

    #[must_use]
    pub fn pending(evidence_ref: impl Into<String>) -> Self {
        Self {
            outcome: CheckOutcome::Pending,
            evidence_ref: evidence_ref.into(),
        }
    }

    fn is_acceptable(&self) -> bool {
        self.outcome.is_passed() && !self.evidence_ref.trim().is_empty()
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
    pub granularity: RequiredCheck,
    pub completeness: RequiredCheck,
    pub nt_mapping: RequiredCheck,
    pub storage: RequiredCheck,
}

impl RequiredChecks {
    fn as_slice(&self) -> [&RequiredCheck; 10] {
        [
            &self.source_access,
            &self.license,
            &self.schema,
            &self.time_semantics,
            &self.instrument_universe,
            &self.coverage,
            &self.granularity,
            &self.completeness,
            &self.nt_mapping,
            &self.storage,
        ]
    }

    /// Names of checks that are not acceptable (failed, pending, or missing
    /// evidence), in declaration order. Empty when every check passed.
    #[must_use]
    pub fn unmet(&self) -> Vec<&'static str> {
        const NAMES: [&str; 10] = [
            "source_access",
            "license",
            "schema",
            "time_semantics",
            "instrument_universe",
            "coverage",
            "granularity",
            "completeness",
            "nt_mapping",
            "storage",
        ];
        self.as_slice()
            .iter()
            .zip(NAMES)
            .filter(|(check, _)| !check.is_acceptable())
            .map(|(_, name)| name)
            .collect()
    }

    /// True only when every required check passed with non-empty evidence.
    #[must_use]
    pub fn all_passed(&self) -> bool {
        self.unmet().is_empty()
    }
}

/// Inclusive-start, exclusive-end UTC time range (RFC 3339 strings).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimeRange {
    pub start_utc: String,
    pub end_utc: String,
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
    pub fixture_type: FixtureType,
    pub requested_time_range: TimeRange,
    pub coverage_time_range: TimeRange,
    pub instrument_universe_id: String,
    pub raw_sample_uri: String,
    pub raw_sample_hash: String,
    pub schema_sample_uri: String,
    pub schema_sample_hash: String,
    pub license_ref: String,
    pub retention_ref: String,
    pub nt_mapping_status: NtMappingStatus,
    pub fidelity_class: SourceProofFidelityClass,
    pub forbidden_claims: Vec<String>,
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
    /// A version field does not equal the contract/schema version this module
    /// implements, so the proof was written against a different contract.
    UnexpectedVersion {
        field: &'static str,
        expected: &'static str,
        actual: String,
    },
    /// The proof's NautilusTrader catalog-mapping status is not `Accepted`.
    NtMappingNotAccepted(NtMappingStatus),
    /// One or more required checks did not pass.
    UnmetChecks(Vec<&'static str>),
    /// The lower-fidelity source cannot carry an execution-quality claim.
    ForbiddenClaimMissing,
    /// The proof referenced by the dataset is not accepted.
    ProofNotAccepted(SourceProofStatus),
    /// A rejected proof cannot satisfy acceptance invariants.
    ProofRejected,
    /// The manifest payload record lacks a required field.
    ManifestRecordIncomplete(&'static str),
    /// The verified object hash does not match the manifest record hash.
    ContentHashMismatch { expected: String, actual: String },
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
}

impl std::fmt::Display for AcceptanceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingField(field) => write!(f, "missing required field: {field}"),
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
            Self::UnmetChecks(checks) => write!(f, "unmet required checks: {}", checks.join(", ")),
            Self::ForbiddenClaimMissing => {
                write!(f, "non-L2 fidelity requires explicit forbidden claims")
            }
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
        }
    }
}

impl std::error::Error for AcceptanceError {}

impl SourceProofReport {
    fn check_required_identity(&self) -> Result<(), AcceptanceError> {
        let required: [(&'static str, &str); 15] = [
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
        if self.status == SourceProofStatus::Rejected {
            return Err(AcceptanceError::ProofRejected);
        }
        self.check_required_identity()?;
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
        mut self,
        mode: AcceptanceMode,
        accepted_by: impl Into<String>,
        accepted_at_utc: impl Into<String>,
    ) -> Result<Self, AcceptanceError> {
        // Only a pending proof may be promoted: an already-rejected (or
        // already-accepted) record must not be silently re-promoted.
        if self.status != SourceProofStatus::Pending {
            return Err(AcceptanceError::NotPending(self.status));
        }
        self.evaluate_acceptance()?;
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
    pub(crate) instrument_universe_id: String,
    pub(crate) fidelity_class: SourceProofFidelityClass,
    pub(crate) forbidden_claims: Vec<String>,
    pub(crate) acceptance_mode: AcceptanceMode,
    pub(crate) accepted_by: String,
    pub(crate) accepted_at: String,
    pub(crate) accepted_object_sha256: String,
    pub(crate) object: IngestManifestObjectRecord,
    _accepted_gate: AcceptedGate,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AcceptedGate;

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
    if !proof.is_accepted() {
        return Err(AcceptanceError::ProofNotAccepted(proof.status));
    }
    // Defence in depth: re-evaluate the acceptance invariants even for a record
    // that already claims accepted status, so a hand-edited record cannot slip
    // through.
    proof.evaluate_acceptance()?;
    object.check_complete()?;

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
    if !source_url_matches_declared_source(&object.source_url, &proof.source_binding, &proof.venue)
    {
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
        instrument_universe_id: proof.instrument_universe_id.clone(),
        fidelity_class: proof.fidelity_class,
        forbidden_claims: proof.forbidden_claims.clone(),
        acceptance_mode,
        accepted_by: accepted_by.clone(),
        accepted_at: accepted_at.clone(),
        accepted_object_sha256: object.sha256.clone(),
        object: object.clone(),
        _accepted_gate: AcceptedGate,
    })
}

fn source_url_matches_declared_source(source_url: &str, source_binding: &str, venue: &str) -> bool {
    let Some(declared_source_uri) = source_binding_source_uri(source_binding, venue) else {
        return false;
    };
    let Some(declared_host) = https_host(&declared_source_uri) else {
        return false;
    };
    let Some(object_host) = https_host(source_url) else {
        return false;
    };
    object_host.eq_ignore_ascii_case(declared_host)
}

fn source_binding_source_uri(source_binding: &str, venue: &str) -> Option<String> {
    let source_binding = source_binding.trim();
    let venue = venue.trim();
    if source_binding.is_empty() || venue.is_empty() {
        return None;
    }
    toml::from_str::<SourceBindingRegistry>(SOURCE_BINDINGS_REGISTRY)
        .ok()?
        .source_bindings
        .into_iter()
        // Binding keys are canonical config IDs; venue labels are operator-facing names.
        .find(|binding| binding.key == source_binding && binding.venue.eq_ignore_ascii_case(venue))
        .map(|binding| binding.source_uri)
}

fn https_host(source_url: &str) -> Option<&str> {
    let (scheme, after_scheme) = source_url.split_once("://")?;
    if !scheme.eq_ignore_ascii_case("https") {
        return None;
    }
    let authority = after_scheme
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default()
        .trim();
    let host = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host)
        .split(':')
        .next()
        .unwrap_or_default()
        .trim_matches(['[', ']'])
        .trim_matches('.');
    if host.is_empty() { None } else { Some(host) }
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
            granularity: RequiredCheck::passed("native_trade_prints"),
            nt_mapping: RequiredCheck::passed("nt://TradeTick"),
            completeness: RequiredCheck::passed(evidence),
            storage: RequiredCheck::passed("s3://bolt-parquet/.../source-proofs/"),
        }
    }

    fn candidate_proof() -> SourceProofReport {
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
            retention_ref: "https://public.bybit.com/ (archive retention reviewed)".to_string(),
            nt_mapping_status: NtMappingStatus::Accepted,
            fidelity_class: SourceProofFidelityClass::TradeReplay,
            forbidden_claims: vec![
                "No execution-quality, queue-position, or order-book-liquidity claims.".to_string(),
            ],
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

    #[test]
    fn all_checks_passed_when_every_check_passes() {
        assert!(passing_checks().all_passed());
        assert!(passing_checks().unmet().is_empty());
    }

    #[test]
    fn pending_check_is_reported_as_unmet() {
        let mut checks = passing_checks();
        checks.license = RequiredCheck::pending("manual review outstanding");
        assert!(!checks.all_passed());
        assert_eq!(checks.unmet(), vec!["license"]);
    }

    #[test]
    fn check_with_empty_evidence_is_unmet() {
        let mut checks = passing_checks();
        checks.nt_mapping = RequiredCheck::passed("");
        assert_eq!(checks.unmet(), vec!["nt_mapping"]);
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
    fn select_rejects_unknown_source_binding() {
        let mut candidate = candidate_proof();
        candidate.source_binding = "bybit-does-not-exist".to_string();
        let accepted = candidate
            .accept(AcceptanceMode::Manual, "operator", "2026-06-02T00:00:00Z")
            .unwrap();
        let object = manifest_object();
        let err = select_accepted_dataset(&accepted, &object, &object.sha256).unwrap_err();
        assert!(
            matches!(err, AcceptanceError::SourceVenueMismatch { .. }),
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
    fn acceptance_blocked_when_schema_sample_hash_missing() {
        let mut proof = candidate_proof();
        proof.schema_sample_hash = "  ".to_string();
        assert_eq!(
            proof.evaluate_acceptance().unwrap_err(),
            AcceptanceError::MissingField("schema_sample_hash")
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
