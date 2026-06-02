use crate::{
    bolt_v3_kill_switch::KillSwitchState,
    bolt_v3_submit_admission::BoltV3KillSwitchForcedReductionClaim,
};
use nautilus_model::enums::TradingState;
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoltV3KillSwitchActionClass {
    CancelOutstandingRisk,
    FlattenPositions,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoltV3KillSwitchActionDecisionMode {
    DryRunProofOnly,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoltV3KillSwitchActionScope {
    account_ids: Vec<String>,
    instrument_ids: Vec<String>,
}

impl BoltV3KillSwitchActionScope {
    pub fn new(
        account_ids: Vec<String>,
        instrument_ids: Vec<String>,
    ) -> Result<Self, BoltV3KillSwitchActionRouterError> {
        if account_ids.is_empty() || account_ids.iter().any(|value| value.trim().is_empty()) {
            return Err(BoltV3KillSwitchActionRouterError::InvalidScope);
        }
        if instrument_ids.is_empty() || instrument_ids.iter().any(|value| value.trim().is_empty()) {
            return Err(BoltV3KillSwitchActionRouterError::InvalidScope);
        }
        Ok(Self {
            account_ids,
            instrument_ids,
        })
    }

    pub fn account_ids(&self) -> &[String] {
        &self.account_ids
    }

    pub fn instrument_ids(&self) -> &[String] {
        &self.instrument_ids
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoltV3KillSwitchActionRequest {
    pub action_class: BoltV3KillSwitchActionClass,
    pub kill_switch_state: KillSwitchState,
    pub nt_trading_state: TradingState,
    pub action_id: String,
    pub policy_sha256: String,
    pub source_timestamp_unix_nanos: u64,
    pub scope: BoltV3KillSwitchActionScope,
    pub forced_reduction_claim: Option<BoltV3KillSwitchForcedReductionClaim>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoltV3KillSwitchActionDecision {
    action_class: BoltV3KillSwitchActionClass,
    decision_mode: BoltV3KillSwitchActionDecisionMode,
    halt_id: String,
    action_id: String,
    policy_sha256: String,
    source_timestamp_unix_nanos: u64,
    scope: BoltV3KillSwitchActionScope,
    forced_reduction_claim: Option<BoltV3KillSwitchForcedReductionClaim>,
}

impl BoltV3KillSwitchActionDecision {
    pub fn action_class(&self) -> BoltV3KillSwitchActionClass {
        self.action_class
    }

    pub fn decision_mode(&self) -> BoltV3KillSwitchActionDecisionMode {
        self.decision_mode
    }

    pub fn halt_id(&self) -> &str {
        &self.halt_id
    }

    pub fn action_id(&self) -> &str {
        &self.action_id
    }

    pub fn policy_sha256(&self) -> &str {
        &self.policy_sha256
    }

    pub fn source_timestamp_unix_nanos(&self) -> u64 {
        self.source_timestamp_unix_nanos
    }

    pub fn scope(&self) -> &BoltV3KillSwitchActionScope {
        &self.scope
    }

    pub fn forced_reduction_claim(&self) -> Option<&BoltV3KillSwitchForcedReductionClaim> {
        self.forced_reduction_claim.as_ref()
    }

    pub fn live_order_effects(&self) -> &[BoltV3KillSwitchLiveOrderEffect] {
        &[]
    }

    pub fn venue_calls(&self) -> &[BoltV3KillSwitchVenueCall] {
        &[]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoltV3KillSwitchLiveOrderEffect {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoltV3KillSwitchVenueCall {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoltV3KillSwitchActionRouterError {
    MissingActionId,
    InvalidPolicySha256,
    MissingSourceTimestamp,
    InvalidScope,
    NtTradingStateNotReducing,
    KillSwitchStateNotCancelling,
    KillSwitchStateNotFlattening,
    ForcedReductionProofRequired,
    ForcedReductionProofMismatch,
}

pub struct BoltV3KillSwitchActionRouter;

impl BoltV3KillSwitchActionRouter {
    pub fn dry_run_decision(
        request: BoltV3KillSwitchActionRequest,
    ) -> Result<BoltV3KillSwitchActionDecision, BoltV3KillSwitchActionRouterError> {
        validate_action_metadata(&request)?;
        match request.action_class {
            BoltV3KillSwitchActionClass::CancelOutstandingRisk => cancel_decision(request),
            BoltV3KillSwitchActionClass::FlattenPositions => flatten_decision(request),
        }
    }
}

fn validate_action_metadata(
    request: &BoltV3KillSwitchActionRequest,
) -> Result<(), BoltV3KillSwitchActionRouterError> {
    if request.action_id.trim().is_empty() {
        return Err(BoltV3KillSwitchActionRouterError::MissingActionId);
    }
    if !is_sha256_hex_digest(&request.policy_sha256) {
        return Err(BoltV3KillSwitchActionRouterError::InvalidPolicySha256);
    }
    if request.source_timestamp_unix_nanos == 0 {
        return Err(BoltV3KillSwitchActionRouterError::MissingSourceTimestamp);
    }
    Ok(())
}

fn is_sha256_hex_digest(value: &str) -> bool {
    let expected_len = hex::encode(Sha256::digest([])).len();
    value.len() == expected_len && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn cancel_decision(
    request: BoltV3KillSwitchActionRequest,
) -> Result<BoltV3KillSwitchActionDecision, BoltV3KillSwitchActionRouterError> {
    require_reducing(request.nt_trading_state)?;
    let KillSwitchState::Cancelling { halt_id } = request.kill_switch_state.clone() else {
        return Err(BoltV3KillSwitchActionRouterError::KillSwitchStateNotCancelling);
    };
    Ok(proof_only_decision(request, halt_id, None))
}

fn flatten_decision(
    request: BoltV3KillSwitchActionRequest,
) -> Result<BoltV3KillSwitchActionDecision, BoltV3KillSwitchActionRouterError> {
    require_reducing(request.nt_trading_state)?;
    let KillSwitchState::Flattening { halt_id } = request.kill_switch_state.clone() else {
        return Err(BoltV3KillSwitchActionRouterError::KillSwitchStateNotFlattening);
    };
    let Some(claim) = request.forced_reduction_claim.clone() else {
        return Err(BoltV3KillSwitchActionRouterError::ForcedReductionProofRequired);
    };
    if claim.halt_id() != halt_id
        || claim.action_id() != request.action_id
        || claim.policy_sha256() != request.policy_sha256
    {
        return Err(BoltV3KillSwitchActionRouterError::ForcedReductionProofMismatch);
    }
    Ok(proof_only_decision(request, halt_id, Some(claim)))
}

fn require_reducing(
    nt_trading_state: TradingState,
) -> Result<(), BoltV3KillSwitchActionRouterError> {
    if nt_trading_state != TradingState::Reducing {
        return Err(BoltV3KillSwitchActionRouterError::NtTradingStateNotReducing);
    }
    Ok(())
}

fn proof_only_decision(
    request: BoltV3KillSwitchActionRequest,
    halt_id: String,
    forced_reduction_claim: Option<BoltV3KillSwitchForcedReductionClaim>,
) -> BoltV3KillSwitchActionDecision {
    BoltV3KillSwitchActionDecision {
        action_class: request.action_class,
        decision_mode: BoltV3KillSwitchActionDecisionMode::DryRunProofOnly,
        halt_id,
        action_id: request.action_id,
        policy_sha256: request.policy_sha256,
        source_timestamp_unix_nanos: request.source_timestamp_unix_nanos,
        scope: request.scope,
        forced_reduction_claim,
    }
}
