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

use serde::{Deserialize, Serialize};

/// Governing backfill table contract version for this slice.
pub const CONTRACT_VERSION: &str = "backfill-table-contract.v1";

/// Source-proof schema version implemented by this module.
pub const SOURCE_PROOF_SCHEMA_VERSION: &str = "backfill-source-proof.v1";

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
    /// One or more required checks did not pass.
    UnmetChecks(Vec<&'static str>),
    /// The lower-fidelity source cannot carry an execution-quality claim.
    ForbiddenClaimMissing,
    /// The proof referenced by the dataset is not accepted.
    ProofNotAccepted(SourceProofStatus),
    /// The manifest payload record lacks a required field.
    ManifestRecordIncomplete(&'static str),
    /// The verified object hash does not match the manifest record hash.
    ContentHashMismatch { expected: String, actual: String },
    /// The selected object lies outside the proof's proven coverage window.
    OutsideCoverage { object_date: String },
}

impl std::fmt::Display for AcceptanceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingField(field) => write!(f, "missing required field: {field}"),
            Self::UnmetChecks(checks) => write!(f, "unmet required checks: {}", checks.join(", ")),
            Self::ForbiddenClaimMissing => {
                write!(f, "non-L2 fidelity requires explicit forbidden claims")
            }
            Self::ProofNotAccepted(status) => {
                write!(f, "source proof is not accepted (status: {status:?})")
            }
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
        }
    }
}

impl std::error::Error for AcceptanceError {}

impl SourceProofReport {
    fn check_required_identity(&self) -> Result<(), AcceptanceError> {
        let required: [(&'static str, &str); 13] = [
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
            ("license_ref", &self.license_ref),
        ];
        for (name, value) in required {
            if value.trim().is_empty() {
                return Err(AcceptanceError::MissingField(name));
            }
        }
        if self.source_proof_version == 0 {
            return Err(AcceptanceError::MissingField("source_proof_version"));
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
        self.check_required_identity()?;
        let unmet = self.required_checks.unmet();
        if !unmet.is_empty() {
            return Err(AcceptanceError::UnmetChecks(unmet));
        }
        if self.fidelity_class != SourceProofFidelityClass::L2Replay
            && self.forbidden_claims.is_empty()
        {
            return Err(AcceptanceError::ForbiddenClaimMissing);
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
        self.evaluate_acceptance()?;
        self.status = SourceProofStatus::Accepted;
        self.acceptance_mode = Some(mode);
        self.accepted_by = Some(accepted_by.into());
        self.accepted_at = Some(accepted_at_utc.into());
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
    pub source_proof_id: String,
    pub source_proof_version: u32,
    pub source_binding: String,
    pub venue: String,
    pub product_family: String,
    pub product_category: String,
    pub instrument_universe_id: String,
    pub fidelity_class: SourceProofFidelityClass,
    pub forbidden_claims: Vec<String>,
    pub object: IngestManifestObjectRecord,
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

    if !date_within(
        &object.archive_date,
        &proof.coverage_time_range.start_utc,
        &proof.coverage_time_range.end_utc,
    ) {
        return Err(AcceptanceError::OutsideCoverage {
            object_date: object.archive_date.clone(),
        });
    }

    Ok(AcceptedDataset {
        source_proof_id: proof.source_proof_id.clone(),
        source_proof_version: proof.source_proof_version,
        source_binding: proof.source_binding.clone(),
        venue: proof.venue.clone(),
        product_family: proof.product_family.clone(),
        product_category: proof.product_category.clone(),
        instrument_universe_id: proof.instrument_universe_id.clone(),
        fidelity_class: proof.fidelity_class,
        forbidden_claims: proof.forbidden_claims.clone(),
        object: object.clone(),
    })
}

/// True when `object_date` (YYYY-MM-DD) falls in `[start_utc, end_utc)`.
///
/// The comparison uses the date prefix of each RFC 3339 bound, which is correct
/// for day-partitioned archive objects: the start date is inclusive and the end
/// date is exclusive.
fn date_within(object_date: &str, start_utc: &str, end_utc: &str) -> bool {
    let start_date = start_utc.get(0..10).unwrap_or(start_utc);
    let end_date = end_utc.get(0..10).unwrap_or(end_utc);
    object_date >= start_date && object_date < end_date
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
    fn acceptance_blocked_when_any_check_pending() {
        let mut proof = candidate_proof();
        proof.required_checks.coverage = RequiredCheck::pending("coverage not proven");
        let err = proof.evaluate_acceptance().unwrap_err();
        assert_eq!(err, AcceptanceError::UnmetChecks(vec!["coverage"]));
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
