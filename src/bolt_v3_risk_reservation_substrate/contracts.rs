use std::{
    collections::{BTreeMap, BTreeSet},
    num::NonZeroU64,
};

use nautilus_model::identifiers::ClientOrderId;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::bolt_v3_risk_reservation_substrate::risk_classifier::{
    ConcentrationBucket, RiskClassificationPolicy, RiskDescriptorCanonicalAttributes,
};

const INITIAL_RISK_STATE_VERSION_VALUE: u64 = u64::MIN;
const MONOTONIC_COUNTER_STEP: u64 = NonZeroU64::MIN.get();
const INITIAL_FENCING_TOKEN_VALUE: u64 = NonZeroU64::MIN.get();

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PoolId(String);

impl PoolId {
    pub fn new(value: impl Into<String>) -> Result<Self, ContractIdentityError> {
        let value = value.into();
        let trimmed = value.trim();
        if trimmed.is_empty() || trimmed != value {
            return Err(ContractIdentityError::InvalidPoolId);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct OwnerId(String);

impl OwnerId {
    pub fn new(value: impl Into<String>) -> Result<Self, ContractIdentityError> {
        let value = value.into();
        let trimmed = value.trim();
        if trimmed.is_empty() || trimmed != value {
            return Err(ContractIdentityError::InvalidOwnerId);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RiskStateVersion(u64);

impl RiskStateVersion {
    pub const fn zero() -> Self {
        Self(INITIAL_RISK_STATE_VERSION_VALUE)
    }

    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }

    pub fn next(self) -> Result<Self, RiskStateVersionError> {
        self.0
            .checked_add(MONOTONIC_COUNTER_STEP)
            .map(Self)
            .ok_or(RiskStateVersionError::Overflow)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FencingToken(u64);

impl FencingToken {
    pub const fn initial() -> Self {
        Self(INITIAL_FENCING_TOKEN_VALUE)
    }

    pub fn new(value: u64) -> Result<Self, FencingTokenError> {
        if value == 0 {
            return Err(FencingTokenError::Zero);
        }
        Ok(Self(value))
    }

    pub const fn get(self) -> u64 {
        self.0
    }

    pub fn next(self) -> Result<Self, FencingTokenError> {
        self.0
            .checked_add(MONOTONIC_COUNTER_STEP)
            .ok_or(FencingTokenError::Overflow)
            .and_then(Self::new)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LeaseAuthorityBackend {
    DynamoDbConditionalWrite,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfiguredLeaseAuthority {
    backend: LeaseAuthorityBackend,
    dependency_name: String,
}

impl ConfiguredLeaseAuthority {
    pub fn new(
        backend: LeaseAuthorityBackend,
        dependency_name: impl Into<String>,
    ) -> Result<Self, LeaseAuthorityConfigError> {
        let dependency_name = dependency_name.into();
        let trimmed = dependency_name.trim();
        if trimmed.is_empty() || trimmed != dependency_name {
            return Err(LeaseAuthorityConfigError::InvalidDependencyName);
        }
        Ok(Self {
            backend,
            dependency_name,
        })
    }

    pub const fn backend(&self) -> LeaseAuthorityBackend {
        self.backend
    }

    pub fn dependency_name(&self) -> &str {
        &self.dependency_name
    }
}

impl<'de> Deserialize<'de> for ConfiguredLeaseAuthority {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            backend: LeaseAuthorityBackend,
            dependency_name: String,
        }

        let wire = Wire::deserialize(deserializer)?;
        Self::new(wire.backend, wire.dependency_name).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RiskReservationSubstrateConfig {
    pub enabled: bool,
    pub pool_lease_authority: ConfiguredLeaseAuthority,
    pub work_bounds: RiskReservationWorkBounds,
    pub offered_load_envelope: Option<RiskReservationOfferedLoadEnvelope>,
}

/// Optional substrate-owned overload envelope for risk-increasing admissions.
///
/// When configured, the substrate enforces the maximum number of in-flight
/// risk-increasing admissions inside the same compare-and-reserve mutex that
/// owns risk-state versioning. The async runtime owns the bounded queue and
/// fairness policy; this substrate boundary only owns the fail-closed admission
/// decision, priority invariant, and operational alert.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RiskReservationOfferedLoadEnvelope {
    max_supported_in_flight_risk_increasing_admissions: u64,
}

impl RiskReservationOfferedLoadEnvelope {
    pub fn new(
        max_supported_in_flight_risk_increasing_admissions: u64,
    ) -> Result<Self, RiskReservationOfferedLoadEnvelopeError> {
        if max_supported_in_flight_risk_increasing_admissions == 0 {
            return Err(
                RiskReservationOfferedLoadEnvelopeError::ZeroMaxSupportedInFlightRiskIncreasingAdmissions,
            );
        }
        Ok(Self {
            max_supported_in_flight_risk_increasing_admissions,
        })
    }

    pub const fn max_supported_in_flight_risk_increasing_admissions(self) -> u64 {
        self.max_supported_in_flight_risk_increasing_admissions
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RiskReservationOfferedLoadEnvelopeError {
    ZeroMaxSupportedInFlightRiskIncreasingAdmissions,
}

impl<'de> Deserialize<'de> for RiskReservationOfferedLoadEnvelope {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            max_supported_in_flight_risk_increasing_admissions: u64,
        }

        let wire = Wire::deserialize(deserializer)?;
        Self::new(wire.max_supported_in_flight_risk_increasing_admissions)
            .map_err(|error| serde::de::Error::custom(format!("{error:?}")))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RiskReservationWorkBounds {
    max_current_position_count: usize,
    max_buckets_per_exposure: usize,
    max_terminal_cash_flow_count_per_exposure: usize,
}

impl RiskReservationWorkBounds {
    pub fn new(
        max_current_position_count: usize,
        max_buckets_per_exposure: usize,
        max_terminal_cash_flow_count_per_exposure: usize,
    ) -> Result<Self, RiskReservationWorkBoundsError> {
        if max_current_position_count == 0 {
            return Err(RiskReservationWorkBoundsError::ZeroCurrentPositionCount);
        }
        if max_buckets_per_exposure == 0 {
            return Err(RiskReservationWorkBoundsError::ZeroBucketsPerExposure);
        }
        if max_terminal_cash_flow_count_per_exposure == 0 {
            return Err(RiskReservationWorkBoundsError::ZeroTerminalCashFlowCountPerExposure);
        }
        Ok(Self {
            max_current_position_count,
            max_buckets_per_exposure,
            max_terminal_cash_flow_count_per_exposure,
        })
    }

    pub const fn max_current_position_count(self) -> usize {
        self.max_current_position_count
    }

    pub const fn max_buckets_per_exposure(self) -> usize {
        self.max_buckets_per_exposure
    }

    pub const fn max_terminal_cash_flow_count_per_exposure(self) -> usize {
        self.max_terminal_cash_flow_count_per_exposure
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RiskReservationWorkBoundsError {
    ZeroCurrentPositionCount,
    ZeroBucketsPerExposure,
    ZeroTerminalCashFlowCountPerExposure,
}

impl<'de> Deserialize<'de> for RiskReservationWorkBounds {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            max_current_position_count: usize,
            max_buckets_per_exposure: usize,
            max_terminal_cash_flow_count_per_exposure: usize,
        }

        let wire = Wire::deserialize(deserializer)?;
        Self::new(
            wire.max_current_position_count,
            wire.max_buckets_per_exposure,
            wire.max_terminal_cash_flow_count_per_exposure,
        )
        .map_err(|error| serde::de::Error::custom(format!("{error:?}")))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PoolOwnershipLease {
    pool_id: PoolId,
    owner_id: OwnerId,
    fencing_token: FencingToken,
    lease_authority: ConfiguredLeaseAuthority,
}

impl PoolOwnershipLease {
    pub fn new(
        pool_id: PoolId,
        owner_id: OwnerId,
        fencing_token: FencingToken,
        lease_authority: ConfiguredLeaseAuthority,
    ) -> Self {
        Self {
            pool_id,
            owner_id,
            fencing_token,
            lease_authority,
        }
    }

    pub fn pool_id(&self) -> &PoolId {
        &self.pool_id
    }

    pub fn owner_id(&self) -> &OwnerId {
        &self.owner_id
    }

    pub const fn fencing_token(&self) -> FencingToken {
        self.fencing_token
    }

    pub fn lease_authority(&self) -> &ConfiguredLeaseAuthority {
        &self.lease_authority
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdmissionCandidate {
    pub intent_id: String,
    pub idempotency_key: String,
    pub pool_id: PoolId,
    pub instrument_id: String,
    pub model_risk_scope: ModelRiskEvaluationScope,
    pub expected_descriptor_version: String,
    pub side: String,
    pub quantity: Decimal,
    pub order_type: String,
    pub time_in_force: String,
    pub max_unit_price: Option<Decimal>,
    pub max_cash_outlay: Decimal,
    pub venue_model_version: String,
    pub fee_model_version: String,
    pub source_view_version: RiskStateVersion,
    pub policy_epoch_id: String,
    pub signal_binding: String,
    pub model_binding: String,
    pub attestation_binding: String,
    pub sizing_permit: SizingDecisionPermit,
    pub expires_at_unix_nanos: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RiskSizingView {
    pub risk_state_version: RiskStateVersion,
    pub reconciliation_ready: bool,
    pub reference_growth_wealth: Decimal,
    pub conservative_liquidation_equity: Decimal,
    pub free_collateral: Decimal,
    pub equity_floor_headroom: Decimal,
    pub governor_headroom: Decimal,
    pub global_stress_loss_headroom: Decimal,
    pub bucket_stress_loss_headrooms: BTreeMap<ConcentrationBucket, Decimal>,
    pub open_order_headroom: u64,
    pub position_quantity_headroom: Decimal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelRiskEvaluationScope {
    CandidateInstrument { instrument_id: String },
    ConcentrationBucket(ConcentrationBucket),
    Portfolio { scope_id: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveDescriptorView {
    pub instrument_id: String,
    pub descriptor_version: String,
    pub policy_epoch_id: String,
    pub terminal_state_ids: Vec<String>,
    pub terminal_cash_flows: Vec<Decimal>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RiskPreviewInput {
    pub pool_id: PoolId,
    pub instrument_id: String,
    pub model_risk_scope: ModelRiskEvaluationScope,
    pub side: String,
    pub quantity: Decimal,
    pub order_type: String,
    pub time_in_force: String,
    pub max_unit_price: Option<Decimal>,
    pub max_cash_outlay: Decimal,
    pub source_view_version: RiskStateVersion,
    pub policy_epoch_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RiskAssessment {
    pub risk_state_version: RiskStateVersion,
    pub accepted: bool,
    pub collateral_required: Decimal,
    pub equity_floor_stress_loss: Decimal,
    pub current_scope_equity_floor_stress_loss: Decimal,
    pub post_candidate_scope_equity_floor_stress_loss: Decimal,
    pub governor_realized_loss: Decimal,
    pub rejection_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdmissionToken {
    pub token_id: String,
    pub pool_id: PoolId,
    pub risk_state_version: RiskStateVersion,
    pub policy_epoch_id: String,
    pub reservation_id: String,
    pub expires_at_unix_nanos: u64,
}

#[derive(Debug, PartialEq, Eq)]
pub struct AdmittedOrder {
    admission_token: AdmissionToken,
    client_order_id: ClientOrderId,
    instrument_id: String,
    submitted_risk_state_version: RiskStateVersion,
}

impl AdmittedOrder {
    pub(super) fn from_submitted_reservation(
        admission_token: AdmissionToken,
        client_order_id: ClientOrderId,
        instrument_id: String,
        submitted_risk_state_version: RiskStateVersion,
    ) -> Self {
        Self {
            admission_token,
            client_order_id,
            instrument_id,
            submitted_risk_state_version,
        }
    }

    pub fn admission_token(&self) -> &AdmissionToken {
        &self.admission_token
    }

    pub fn client_order_id(&self) -> ClientOrderId {
        self.client_order_id
    }

    pub fn instrument_id(&self) -> &str {
        &self.instrument_id
    }

    pub fn risk_state_version(&self) -> RiskStateVersion {
        self.submitted_risk_state_version
    }

    pub fn idempotency_key(&self) -> &str {
        &self.admission_token.token_id
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SizingDecisionPermit {
    pub permit_id: String,
    pub source_view_version: RiskStateVersion,
    pub candidate_digest: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReservationLifecycleState {
    Reserved,
    Submitted,
    Open,
    PartiallyFilled,
    Filled,
    Settled,
    CancelRequested,
    CancelConfirmed,
    ExpiredConfirmed,
    SubmissionUnknown,
    ReconciliationRequired,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DurableSubmissionIntent {
    pub admission_token: AdmissionToken,
    pub client_order_id: ClientOrderId,
    pub instrument_id: String,
    pub persisted_at_unix_nanos: u64,
    pub submitted_risk_state_version: RiskStateVersion,
}

impl DurableSubmissionIntent {
    pub fn idempotency_key(&self) -> &str {
        &self.admission_token.token_id
    }

    pub fn client_order_id(&self) -> ClientOrderId {
        self.client_order_id
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveSubmissionRecord {
    pub client_order_id: ClientOrderId,
    pub risk_state_version: RiskStateVersion,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SafetyAction {
    CancelExistingOrder { client_order_id: String },
    ReduceOnlyCloseExistingPosition { position_id: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedPolicyEpoch {
    pub epoch_id: String,
    pub environment: String,
    pub pool_id: PoolId,
    pub policy_digest: String,
    pub descriptor_map_digest: String,
    pub descriptor_map: BTreeMap<String, PreparedEpochDescriptor>,
    pub classifier_version: String,
    pub classification_policy: RiskClassificationPolicy,
    pub model_version: String,
    pub fallback_model_version: String,
    pub fee_model_version: String,
    pub sizing_policy_versions: Vec<String>,
    pub approvals: Vec<PolicyApproval>,
    pub approval_digest: String,
    pub declared_attestations: Vec<PreparedEpochAttestation>,
    pub activation_not_after_unix_nanos: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedEpochDescriptor {
    pub active_descriptor: ActiveDescriptorView,
    pub descriptor_attributes: RiskDescriptorCanonicalAttributes,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyApproval {
    pub approval_id: String,
    pub approver_id: String,
    pub approved_at_unix_nanos: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreparedEpochAttestation {
    BandCoverageAttestation {
        attestation_digest: String,
        artifact: Option<BandCoverageAttestation>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BandCoverageAttestation {
    pub content_digest: String,
    pub producer_identity: String,
    pub certifier_identity: String,
    pub decision: BandCoverageAttestationDecision,
    pub evidence: BandCoverageAttestationEvidence,
    pub valid_from_unix_nanos: u64,
    pub valid_until_unix_nanos: u64,
    pub revoked: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum BandCoverageAttestationDecision {
    Approved,
    Rejected,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BandCoverageAttestationEvidence {
    pub evidence_digest: String,
    pub model_version: String,
    pub band_method_version: String,
    pub market_segment_id: String,
    pub side: String,
    pub decision_horizon: String,
    pub evaluation_cutoff_unix_nanos: u64,
    pub dataset_digest: String,
    pub certified_property: String,
    pub certified_bound_end: String,
    pub confidence_method: String,
    pub multiplicity_method: String,
    pub eligibility_policy_version: String,
    pub eligibility_passed: bool,
    pub outcome_space_id: String,
    pub outcome_space_version: String,
    pub outcome_definition_id: String,
    pub outcome_definition_version: String,
    pub forecast_record_schema_version: String,
    pub evaluation_implementation_version: String,
    pub dependence_inference_method_version: String,
    pub attestation_schema_version: String,
    pub cells: Vec<BandCoverageAttestationCellEvidence>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BandCoverageAttestationCellEvidence {
    pub cell_id: String,
    pub observed_mean_residual: Decimal,
    pub lower_confidence_bound: Decimal,
    pub effective_event_count: u64,
    pub minimum_effective_event_count: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BandCoverageAttestationDigestError {
    CanonicalDigestUnavailable,
}

impl BandCoverageAttestation {
    pub fn canonical_digest(&self) -> Result<String, BandCoverageAttestationDigestError> {
        let input = BandCoverageAttestationDigestInput {
            content_digest: &self.content_digest,
            producer_identity: &self.producer_identity,
            certifier_identity: &self.certifier_identity,
            decision: self.decision,
            evidence: BandCoverageAttestationEvidenceDigestInput {
                evidence_digest: &self.evidence.evidence_digest,
                model_version: &self.evidence.model_version,
                band_method_version: &self.evidence.band_method_version,
                market_segment_id: &self.evidence.market_segment_id,
                side: &self.evidence.side,
                decision_horizon: &self.evidence.decision_horizon,
                evaluation_cutoff_unix_nanos: self.evidence.evaluation_cutoff_unix_nanos,
                dataset_digest: &self.evidence.dataset_digest,
                certified_property: &self.evidence.certified_property,
                certified_bound_end: &self.evidence.certified_bound_end,
                confidence_method: &self.evidence.confidence_method,
                multiplicity_method: &self.evidence.multiplicity_method,
                eligibility_policy_version: &self.evidence.eligibility_policy_version,
                eligibility_passed: self.evidence.eligibility_passed,
                outcome_space_id: &self.evidence.outcome_space_id,
                outcome_space_version: &self.evidence.outcome_space_version,
                outcome_definition_id: &self.evidence.outcome_definition_id,
                outcome_definition_version: &self.evidence.outcome_definition_version,
                forecast_record_schema_version: &self.evidence.forecast_record_schema_version,
                evaluation_implementation_version: &self.evidence.evaluation_implementation_version,
                dependence_inference_method_version: &self
                    .evidence
                    .dependence_inference_method_version,
                attestation_schema_version: &self.evidence.attestation_schema_version,
                cells: self
                    .evidence
                    .cells
                    .iter()
                    .map(|cell| BandCoverageAttestationCellDigestInput {
                        cell_id: &cell.cell_id,
                        observed_mean_residual: cell.observed_mean_residual.to_string(),
                        lower_confidence_bound: cell.lower_confidence_bound.to_string(),
                        effective_event_count: cell.effective_event_count,
                        minimum_effective_event_count: cell.minimum_effective_event_count,
                    })
                    .collect(),
            },
            valid_from_unix_nanos: self.valid_from_unix_nanos,
            valid_until_unix_nanos: self.valid_until_unix_nanos,
            revoked: self.revoked,
        };
        let bytes = serde_json::to_vec(&input)
            .map_err(|_| BandCoverageAttestationDigestError::CanonicalDigestUnavailable)?;
        Ok(hex::encode(Sha256::digest(bytes)))
    }
}

#[derive(Serialize)]
struct BandCoverageAttestationDigestInput<'a> {
    content_digest: &'a str,
    producer_identity: &'a str,
    certifier_identity: &'a str,
    decision: BandCoverageAttestationDecision,
    evidence: BandCoverageAttestationEvidenceDigestInput<'a>,
    valid_from_unix_nanos: u64,
    valid_until_unix_nanos: u64,
    revoked: bool,
}

#[derive(Serialize)]
struct BandCoverageAttestationEvidenceDigestInput<'a> {
    evidence_digest: &'a str,
    model_version: &'a str,
    band_method_version: &'a str,
    market_segment_id: &'a str,
    side: &'a str,
    decision_horizon: &'a str,
    evaluation_cutoff_unix_nanos: u64,
    dataset_digest: &'a str,
    certified_property: &'a str,
    certified_bound_end: &'a str,
    confidence_method: &'a str,
    multiplicity_method: &'a str,
    eligibility_policy_version: &'a str,
    eligibility_passed: bool,
    outcome_space_id: &'a str,
    outcome_space_version: &'a str,
    outcome_definition_id: &'a str,
    outcome_definition_version: &'a str,
    forecast_record_schema_version: &'a str,
    evaluation_implementation_version: &'a str,
    dependence_inference_method_version: &'a str,
    attestation_schema_version: &'a str,
    cells: Vec<BandCoverageAttestationCellDigestInput<'a>>,
}

#[derive(Serialize)]
struct BandCoverageAttestationCellDigestInput<'a> {
    cell_id: &'a str,
    observed_mean_residual: String,
    lower_confidence_bound: String,
    effective_event_count: u64,
    minimum_effective_event_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SafetyPolicyEnvelope {
    pub envelope_id: String,
    pub envelope_version: String,
    pub environment: String,
    pub pool_id: PoolId,
    pub ranges: SafetyPolicyEnvelopeRanges,
    pub permitted_model_versions: BTreeSet<String>,
    pub permitted_fallback_model_versions: BTreeSet<String>,
    pub permitted_classifier_versions: BTreeSet<String>,
    pub permitted_fee_model_versions: BTreeSet<String>,
    pub permitted_sizing_policy_versions: BTreeSet<String>,
    pub required_approval_ids: BTreeSet<String>,
    pub required_approval_digest: String,
    pub invariants: BTreeSet<SafetyEnvelopeInvariant>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SafetyPolicyEnvelopeRanges {
    pub max_descriptor_count: usize,
    pub max_terminal_states_per_descriptor: usize,
    pub min_terminal_cash_flow: Decimal,
    pub max_terminal_cash_flow: Decimal,
    pub max_sizing_policy_versions: usize,
    pub max_activation_horizon_unix_nanos: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SafetyEnvelopeInvariant {
    DescriptorPolicyEpochMatchesBundle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContractIdentityError {
    InvalidPoolId,
    InvalidOwnerId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RiskStateVersionError {
    Overflow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FencingTokenError {
    Zero,
    Overflow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeaseAuthorityConfigError {
    InvalidDependencyName,
}

impl std::fmt::Display for LeaseAuthorityConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidDependencyName => write!(f, "lease authority dependency name is invalid"),
        }
    }
}
