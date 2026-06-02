use crate::{
    bolt_v3_kill_switch::KillSwitchState,
    bolt_v3_order_intent::{NtOrderBuildInputs, NtOrderTemplate, validate_nt_order_template},
    bolt_v3_position_contract::{expected_exit_order_side_for_position, is_observed_open_side},
    bolt_v3_submit_admission::BoltV3KillSwitchForcedReductionClaim,
};
use nautilus_model::{
    enums::{OrderSide, PositionSide, TradingState},
    identifiers::{AccountId, ClientOrderId, InstrumentId, PositionId, StrategyId},
    types::Quantity,
};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoltV3KillSwitchFlattenPositionEvidenceKind {
    CachePosition,
    PositionStatusReport,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BoltV3KillSwitchFlattenPositionState {
    pub evidence_kind: BoltV3KillSwitchFlattenPositionEvidenceKind,
    pub account_id: AccountId,
    pub instrument_id: InstrumentId,
    pub strategy_id: StrategyId,
    pub position_id: PositionId,
    pub position_side: PositionSide,
    pub quantity: Quantity,
    pub source_timestamp_unix_nanos: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BoltV3KillSwitchFlattenCandidate {
    evidence_kind: BoltV3KillSwitchFlattenPositionEvidenceKind,
    account_id: AccountId,
    instrument_id: InstrumentId,
    strategy_id: StrategyId,
    position_id: PositionId,
    position_side: PositionSide,
    quantity: Quantity,
    source_timestamp_unix_nanos: u64,
}

impl BoltV3KillSwitchFlattenCandidate {
    pub fn from_nt_position_state(
        position_state: BoltV3KillSwitchFlattenPositionState,
    ) -> Result<Self, BoltV3KillSwitchFlattenError> {
        Ok(Self {
            evidence_kind: position_state.evidence_kind,
            account_id: position_state.account_id,
            instrument_id: position_state.instrument_id,
            strategy_id: position_state.strategy_id,
            position_id: position_state.position_id,
            position_side: position_state.position_side,
            quantity: position_state.quantity,
            source_timestamp_unix_nanos: position_state.source_timestamp_unix_nanos,
        })
    }

    pub fn evidence_kind(&self) -> BoltV3KillSwitchFlattenPositionEvidenceKind {
        self.evidence_kind
    }

    pub fn account_id(&self) -> AccountId {
        self.account_id
    }

    pub fn instrument_id(&self) -> InstrumentId {
        self.instrument_id
    }

    pub fn strategy_id(&self) -> StrategyId {
        self.strategy_id
    }

    pub fn position_id(&self) -> PositionId {
        self.position_id
    }

    pub fn position_side(&self) -> PositionSide {
        self.position_side
    }

    pub fn quantity(&self) -> Quantity {
        self.quantity
    }

    pub fn source_timestamp_unix_nanos(&self) -> u64 {
        self.source_timestamp_unix_nanos
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct BoltV3KillSwitchFlattenSnapshot {
    candidates: Vec<BoltV3KillSwitchFlattenCandidate>,
    open_positions: Vec<BoltV3KillSwitchFlattenCandidate>,
    flat_positions: Vec<BoltV3KillSwitchFlattenCandidate>,
    unknown_side_positions: Vec<BoltV3KillSwitchFlattenCandidate>,
}

impl BoltV3KillSwitchFlattenSnapshot {
    pub fn new(
        candidates: Vec<BoltV3KillSwitchFlattenCandidate>,
    ) -> Result<Self, BoltV3KillSwitchFlattenError> {
        let open_positions = candidates
            .iter()
            .filter(|candidate| is_observed_open_side(candidate.position_side))
            .cloned()
            .collect();
        let flat_positions = candidates
            .iter()
            .filter(|candidate| candidate.position_side == PositionSide::Flat)
            .cloned()
            .collect();
        let unknown_side_positions = candidates
            .iter()
            .filter(|candidate| candidate.position_side == PositionSide::NoPositionSide)
            .cloned()
            .collect();

        Ok(Self {
            candidates,
            open_positions,
            flat_positions,
            unknown_side_positions,
        })
    }

    pub fn candidates(&self) -> &[BoltV3KillSwitchFlattenCandidate] {
        &self.candidates
    }

    pub fn open_positions(&self) -> &[BoltV3KillSwitchFlattenCandidate] {
        &self.open_positions
    }

    pub fn flat_positions(&self) -> &[BoltV3KillSwitchFlattenCandidate] {
        &self.flat_positions
    }

    pub fn unknown_side_positions(&self) -> &[BoltV3KillSwitchFlattenCandidate] {
        &self.unknown_side_positions
    }

    pub fn has_residual_position_risk(&self) -> bool {
        !self.open_positions.is_empty() || !self.unknown_side_positions.is_empty()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BoltV3KillSwitchFlattenPolicy {
    max_source_age_unix_nanos: u64,
}

impl BoltV3KillSwitchFlattenPolicy {
    pub fn with_source_freshness(
        max_source_age_unix_nanos: u64,
    ) -> Result<Self, BoltV3KillSwitchFlattenError> {
        Ok(Self {
            max_source_age_unix_nanos,
        })
    }

    pub fn validate_snapshot(
        &self,
        snapshot: &BoltV3KillSwitchFlattenSnapshot,
        observed_at_unix_nanos: u64,
    ) -> Result<(), BoltV3KillSwitchFlattenError> {
        if snapshot.candidates().is_empty() {
            return Err(BoltV3KillSwitchFlattenError::MissingPositionProof);
        }
        for candidate in snapshot.candidates() {
            let Some(source_age_unix_nanos) =
                observed_at_unix_nanos.checked_sub(candidate.source_timestamp_unix_nanos())
            else {
                return Err(BoltV3KillSwitchFlattenError::StaleSourceTimestamp);
            };
            if source_age_unix_nanos > self.max_source_age_unix_nanos {
                return Err(BoltV3KillSwitchFlattenError::StaleSourceTimestamp);
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoltV3KillSwitchFlattenDecisionMode {
    DryRunProofOnly,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BoltV3KillSwitchFlattenPlanRequest {
    pub kill_switch_state: KillSwitchState,
    pub nt_trading_state: TradingState,
    pub action_id: String,
    pub config_sha256: String,
    pub policy_sha256: String,
    pub source_timestamp_unix_nanos: u64,
    pub policy: BoltV3KillSwitchFlattenPolicy,
    pub snapshot: BoltV3KillSwitchFlattenSnapshot,
    pub observed_at_unix_nanos: u64,
    pub route_proof: BoltV3KillSwitchFlattenRouteProof,
    pub order_template: NtOrderTemplate,
    pub forced_reduction_claim: BoltV3KillSwitchForcedReductionClaim,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BoltV3KillSwitchFlattenPlan {
    halt_id: String,
    decision_mode: BoltV3KillSwitchFlattenDecisionMode,
    candidates: Vec<BoltV3KillSwitchFlattenCandidate>,
    commands: Vec<BoltV3KillSwitchFlattenCommand>,
}

impl BoltV3KillSwitchFlattenPlan {
    pub fn halt_id(&self) -> &str {
        &self.halt_id
    }

    pub fn decision_mode(&self) -> BoltV3KillSwitchFlattenDecisionMode {
        self.decision_mode
    }

    pub fn candidates(&self) -> &[BoltV3KillSwitchFlattenCandidate] {
        &self.candidates
    }

    pub fn commands(&self) -> &[BoltV3KillSwitchFlattenCommand] {
        &self.commands
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoltV3KillSwitchFlattenRouteKind {
    PerStrategyActionPort,
    LiveNodeCommandRouter,
    Unsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BoltV3KillSwitchFlattenRouteProof {
    route_kind: BoltV3KillSwitchFlattenRouteKind,
}

impl BoltV3KillSwitchFlattenRouteProof {
    pub fn new(route_kind: BoltV3KillSwitchFlattenRouteKind) -> Self {
        Self { route_kind }
    }

    pub fn route_kind(&self) -> BoltV3KillSwitchFlattenRouteKind {
        self.route_kind
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct BoltV3KillSwitchFlattenCommand {
    halt_id: String,
    action_id: String,
    config_sha256: String,
    policy_sha256: String,
    source_timestamp_unix_nanos: u64,
    account_id: AccountId,
    instrument_id: InstrumentId,
    strategy_id: StrategyId,
    position_id: PositionId,
    position_side: PositionSide,
    order_side: OrderSide,
    quantity: Quantity,
    order_template: NtOrderTemplate,
    route_kind: BoltV3KillSwitchFlattenRouteKind,
    forced_reduction_claim: BoltV3KillSwitchForcedReductionClaim,
}

impl BoltV3KillSwitchFlattenCommand {
    pub fn halt_id(&self) -> &str {
        &self.halt_id
    }

    pub fn action_id(&self) -> &str {
        &self.action_id
    }

    pub fn config_sha256(&self) -> &str {
        &self.config_sha256
    }

    pub fn policy_sha256(&self) -> &str {
        &self.policy_sha256
    }

    pub fn source_timestamp_unix_nanos(&self) -> u64 {
        self.source_timestamp_unix_nanos
    }

    pub fn account_id(&self) -> AccountId {
        self.account_id
    }

    pub fn instrument_id(&self) -> InstrumentId {
        self.instrument_id
    }

    pub fn strategy_id(&self) -> StrategyId {
        self.strategy_id
    }

    pub fn position_id(&self) -> PositionId {
        self.position_id
    }

    pub fn position_side(&self) -> PositionSide {
        self.position_side
    }

    pub fn order_side(&self) -> OrderSide {
        self.order_side
    }

    pub fn quantity(&self) -> Quantity {
        self.quantity
    }

    pub fn order_template(&self) -> &NtOrderTemplate {
        &self.order_template
    }

    pub fn route_kind(&self) -> BoltV3KillSwitchFlattenRouteKind {
        self.route_kind
    }

    pub fn forced_reduction_claim(&self) -> &BoltV3KillSwitchForcedReductionClaim {
        &self.forced_reduction_claim
    }
}

pub struct BoltV3KillSwitchFlattenSupervisor;

impl BoltV3KillSwitchFlattenSupervisor {
    pub fn plan_flatten(
        request: BoltV3KillSwitchFlattenPlanRequest,
    ) -> Result<BoltV3KillSwitchFlattenPlan, BoltV3KillSwitchFlattenError> {
        let KillSwitchState::Flattening { halt_id } = request.kill_switch_state.clone() else {
            return Err(BoltV3KillSwitchFlattenError::KillSwitchStateNotFlattening);
        };
        if request.nt_trading_state != TradingState::Reducing {
            return Err(BoltV3KillSwitchFlattenError::NtTradingStateNotReducing);
        }
        validate_flatten_command_metadata(&request)?;
        validate_flatten_order_template(
            &request.order_template,
            request.snapshot.open_positions(),
            &request.action_id,
        )?;
        if request.forced_reduction_claim.halt_id() != halt_id
            || request.forced_reduction_claim.action_id() != request.action_id
            || request.forced_reduction_claim.policy_sha256() != request.policy_sha256
        {
            return Err(BoltV3KillSwitchFlattenError::ForcedReductionProofMismatch);
        }
        if request.route_proof.route_kind() == BoltV3KillSwitchFlattenRouteKind::Unsupported {
            return Err(BoltV3KillSwitchFlattenError::UnsupportedRouteProof);
        }
        request
            .policy
            .validate_snapshot(&request.snapshot, request.observed_at_unix_nanos)?;
        let commands = request
            .snapshot
            .open_positions()
            .iter()
            .map(|candidate| BoltV3KillSwitchFlattenCommand {
                halt_id: halt_id.clone(),
                action_id: request.action_id.clone(),
                config_sha256: request.config_sha256.clone(),
                policy_sha256: request.policy_sha256.clone(),
                source_timestamp_unix_nanos: request.source_timestamp_unix_nanos,
                account_id: candidate.account_id(),
                instrument_id: candidate.instrument_id(),
                strategy_id: candidate.strategy_id(),
                position_id: candidate.position_id(),
                position_side: candidate.position_side(),
                order_side: expected_exit_order_side_for_position(candidate.position_side())
                    .expect("open position side should produce an exit order side"),
                quantity: candidate.quantity(),
                order_template: request.order_template.clone(),
                route_kind: request.route_proof.route_kind(),
                forced_reduction_claim: request.forced_reduction_claim.clone(),
            })
            .collect();
        Ok(BoltV3KillSwitchFlattenPlan {
            halt_id,
            decision_mode: BoltV3KillSwitchFlattenDecisionMode::DryRunProofOnly,
            candidates: request.snapshot.candidates().to_vec(),
            commands,
        })
    }
}

fn validate_flatten_command_metadata(
    request: &BoltV3KillSwitchFlattenPlanRequest,
) -> Result<(), BoltV3KillSwitchFlattenError> {
    if request.action_id.trim().is_empty() {
        return Err(BoltV3KillSwitchFlattenError::MissingActionId);
    }
    if !is_sha256_hex(&request.config_sha256) {
        return Err(BoltV3KillSwitchFlattenError::InvalidConfigSha256);
    }
    if !is_sha256_hex(&request.policy_sha256) {
        return Err(BoltV3KillSwitchFlattenError::InvalidPolicySha256);
    }
    if request.source_timestamp_unix_nanos == 0 {
        return Err(BoltV3KillSwitchFlattenError::MissingSourceTimestamp);
    }
    Ok(())
}

fn validate_flatten_order_template(
    template: &NtOrderTemplate,
    open_positions: &[BoltV3KillSwitchFlattenCandidate],
    action_id: &str,
) -> Result<(), BoltV3KillSwitchFlattenError> {
    if !template.is_reduce_only {
        return Err(BoltV3KillSwitchFlattenError::OrderTemplateNotReduceOnly);
    }
    if template.is_quote_quantity {
        return Err(BoltV3KillSwitchFlattenError::OrderTemplateUsesQuoteQuantity);
    }
    for candidate in open_positions {
        let order_side = expected_exit_order_side_for_position(candidate.position_side())
            .expect("open position side should produce an exit order side");
        let inputs = NtOrderBuildInputs {
            instrument_id: candidate.instrument_id(),
            order_side,
            quantity: candidate.quantity(),
            price: None,
            client_order_id: ClientOrderId::from(action_id),
        };
        validate_nt_order_template(action_id, template, &inputs)
            .map_err(|_| BoltV3KillSwitchFlattenError::InvalidOrderTemplate)?;
    }
    Ok(())
}

fn is_sha256_hex(value: &str) -> bool {
    let expected_len = hex::encode(Sha256::digest([])).len();
    value.len() == expected_len && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoltV3KillSwitchFlattenError {
    ForcedReductionProofMismatch,
    InvalidConfigSha256,
    InvalidOrderTemplate,
    InvalidPolicySha256,
    MissingActionId,
    KillSwitchStateNotFlattening,
    MissingCandidates,
    MissingPositionProof,
    MissingSourceTimestamp,
    NtTradingStateNotReducing,
    OrderTemplateNotReduceOnly,
    OrderTemplateUsesQuoteQuantity,
    StaleSourceTimestamp,
    UnsupportedRouteProof,
}
