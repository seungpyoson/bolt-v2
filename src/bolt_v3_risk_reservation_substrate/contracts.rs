use std::num::NonZeroU64;

use rust_decimal::Decimal;
use serde::Deserialize;

const INITIAL_RISK_STATE_VERSION_VALUE: u64 = u64::MIN;
const MONOTONIC_COUNTER_STEP: u64 = NonZeroU64::MIN.get();
const INITIAL_FENCING_TOKEN_VALUE: u64 = NonZeroU64::MIN.get();

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PoolId(String);

impl PoolId {
    pub fn new(value: impl Into<String>) -> Result<Self, ContractIdentityError> {
        let value = value.into();
        if value.trim().is_empty() || value.trim() != value {
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
        if value.trim().is_empty() || value.trim() != value {
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
        if dependency_name.trim().is_empty() || dependency_name.trim() != dependency_name {
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelRiskEvaluationScope {
    pub scope_id: String,
    pub scope_kind: String,
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
    pub reservation_id: String,
    pub expires_at_unix_nanos: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdmittedOrder {
    pub admission_token: AdmissionToken,
    pub client_order_id: String,
    pub instrument_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SizingDecisionPermit {
    pub permit_id: String,
    pub source_view_version: RiskStateVersion,
    pub candidate_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SafetyAction {
    CancelExistingOrder { client_order_id: String },
    ReduceOnlyCloseExistingPosition { position_id: String },
    VenueRequiredAdministrative { action_id: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedPolicyEpoch {
    pub epoch_id: String,
    pub descriptor_map_digest: String,
    pub classifier_version: String,
    pub fee_model_version: String,
    pub sizing_policy_versions: Vec<String>,
    pub approval_digest: String,
    pub attestation_digests: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SafetyPolicyEnvelope {
    pub envelope_id: String,
    pub environment: String,
    pub pool_id: PoolId,
    pub allowed_range_digest: String,
    pub required_approval_digest: String,
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
