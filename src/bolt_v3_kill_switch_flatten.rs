use crate::{
    bolt_v3_kill_switch::KillSwitchState,
    bolt_v3_numeric::is_sha256_hex_digest,
    bolt_v3_order_intent::{NtOrderBuildInputs, NtOrderTemplate, validate_nt_order_template},
    bolt_v3_position_contract::{expected_exit_order_side_for_position, is_observed_open_side},
    bolt_v3_submit_admission::BoltV3KillSwitchForcedReductionClaim,
};
use nautilus_model::{
    enums::{OrderSide, OrderStatus, PositionSide, TradingState},
    identifiers::{AccountId, ClientOrderId, InstrumentId, PositionId, StrategyId},
    types::Quantity,
};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoltV3KillSwitchFlattenPositionEvidenceKind {
    CachePosition,
    PositionStatusReport,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoltV3KillSwitchFlattenQuantitySource {
    CachePositionQuantity,
    PositionStatusReportQuantity,
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
    quantity_source: BoltV3KillSwitchFlattenQuantitySource,
    source_timestamp_unix_nanos: u64,
}

impl BoltV3KillSwitchFlattenCandidate {
    pub fn from_nt_position_state(
        position_state: BoltV3KillSwitchFlattenPositionState,
    ) -> Result<Self, BoltV3KillSwitchFlattenError> {
        let quantity_source = match position_state.evidence_kind {
            BoltV3KillSwitchFlattenPositionEvidenceKind::CachePosition => {
                BoltV3KillSwitchFlattenQuantitySource::CachePositionQuantity
            }
            BoltV3KillSwitchFlattenPositionEvidenceKind::PositionStatusReport => {
                BoltV3KillSwitchFlattenQuantitySource::PositionStatusReportQuantity
            }
        };
        if is_observed_open_side(position_state.position_side) && position_state.quantity.is_zero()
        {
            return Err(BoltV3KillSwitchFlattenError::InconsistentPositionProof);
        }
        if position_state.source_timestamp_unix_nanos == 0 {
            return Err(BoltV3KillSwitchFlattenError::MissingSourceTimestamp);
        }
        Ok(Self {
            evidence_kind: position_state.evidence_kind,
            account_id: position_state.account_id,
            instrument_id: position_state.instrument_id,
            strategy_id: position_state.strategy_id,
            position_id: position_state.position_id,
            position_side: position_state.position_side,
            quantity: position_state.quantity,
            quantity_source,
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

    pub fn quantity_source(&self) -> BoltV3KillSwitchFlattenQuantitySource {
        self.quantity_source
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
        validate_no_conflicting_position_proof(&candidates)?;
        let mut scoped_candidates = BTreeMap::new();
        for candidate in candidates {
            let scoped_position_identity = (
                candidate.account_id,
                candidate.instrument_id,
                candidate.strategy_id,
                candidate.position_id,
            );
            scoped_candidates
                .entry(scoped_position_identity)
                .or_insert(candidate);
        }
        let candidates: Vec<BoltV3KillSwitchFlattenCandidate> =
            scoped_candidates.into_values().collect();
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
        if max_source_age_unix_nanos == 0 {
            return Err(BoltV3KillSwitchFlattenError::NonPositiveSourceFreshness);
        }
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
    quantity_source: BoltV3KillSwitchFlattenQuantitySource,
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

    pub fn quantity_source(&self) -> BoltV3KillSwitchFlattenQuantitySource {
        self.quantity_source
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
        let action_id = request.action_id.trim().to_string();
        validate_flatten_order_template(
            &request.order_template,
            request.snapshot.open_positions(),
            &action_id,
        )?;
        if request.forced_reduction_claim.halt_id() != halt_id
            || request.forced_reduction_claim.action_id().trim() != action_id
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
                action_id: action_id.clone(),
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
                quantity_source: candidate.quantity_source(),
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
    if !is_sha256_hex_digest(&request.config_sha256) {
        return Err(BoltV3KillSwitchFlattenError::InvalidConfigSha256);
    }
    if !is_sha256_hex_digest(&request.policy_sha256) {
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

fn validate_no_conflicting_position_proof(
    candidates: &[BoltV3KillSwitchFlattenCandidate],
) -> Result<(), BoltV3KillSwitchFlattenError> {
    for (index, left) in candidates.iter().enumerate() {
        for right in candidates.iter().skip(index + 1) {
            if left.account_id == right.account_id
                && left.instrument_id == right.instrument_id
                && left.strategy_id == right.strategy_id
                && left.position_id == right.position_id
                && (left.position_side != right.position_side || left.quantity != right.quantity)
            {
                return Err(BoltV3KillSwitchFlattenError::ConflictingPositionProof);
            }
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoltV3KillSwitchFlattenAttemptOutcome {
    SubmitPlanned,
    SubmitAccepted,
    SubmitRejected,
    PartialFill,
    ResidualPositionRemains,
    FlatPositionObserved,
    StalePositionProof,
    UnsupportedInstrument,
    ThinBookNoFillabilityProof,
}

impl BoltV3KillSwitchFlattenAttemptOutcome {
    pub fn submit_planned() -> Self {
        Self::SubmitPlanned
    }

    pub fn submit_accepted(status: OrderStatus) -> Result<Self, BoltV3KillSwitchFlattenError> {
        if status != OrderStatus::Accepted {
            return Err(BoltV3KillSwitchFlattenError::InvalidAttemptOutcome);
        }
        Ok(Self::SubmitAccepted)
    }

    pub fn submit_rejected(status: OrderStatus) -> Result<Self, BoltV3KillSwitchFlattenError> {
        if status != OrderStatus::Rejected {
            return Err(BoltV3KillSwitchFlattenError::InvalidAttemptOutcome);
        }
        Ok(Self::SubmitRejected)
    }

    pub fn partial_fill(status: OrderStatus) -> Result<Self, BoltV3KillSwitchFlattenError> {
        if status != OrderStatus::PartiallyFilled {
            return Err(BoltV3KillSwitchFlattenError::InvalidAttemptOutcome);
        }
        Ok(Self::PartialFill)
    }

    pub fn residual_position_remains(
        position_side: PositionSide,
        source_timestamp_unix_nanos: u64,
    ) -> Result<Self, BoltV3KillSwitchFlattenError> {
        if !is_observed_open_side(position_side) || source_timestamp_unix_nanos == 0 {
            return Err(BoltV3KillSwitchFlattenError::InvalidAttemptOutcome);
        }
        Ok(Self::ResidualPositionRemains)
    }

    pub fn flat_position_observed(
        position_side: PositionSide,
        source_timestamp_unix_nanos: u64,
    ) -> Result<Self, BoltV3KillSwitchFlattenError> {
        if position_side != PositionSide::Flat || source_timestamp_unix_nanos == 0 {
            return Err(BoltV3KillSwitchFlattenError::InvalidAttemptOutcome);
        }
        Ok(Self::FlatPositionObserved)
    }

    pub fn stale_position_proof() -> Self {
        Self::StalePositionProof
    }

    pub fn unsupported_instrument() -> Self {
        Self::UnsupportedInstrument
    }

    pub fn thin_book_no_fillability() -> Self {
        Self::ThinBookNoFillabilityProof
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum BoltV3KillSwitchFlattenAggregateOutcome {
    AllFlat,
    OutstandingFlattenSubmit,
    ResidualPositionRemains,
    FailedManualIntervention,
    SubmitRejectedManualIntervention,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BoltV3KillSwitchFlattenOutcomeSummary {
    aggregate: BoltV3KillSwitchFlattenAggregateOutcome,
    authorizes_durable_state_transition: bool,
}

impl BoltV3KillSwitchFlattenOutcomeSummary {
    pub fn aggregate(&self) -> BoltV3KillSwitchFlattenAggregateOutcome {
        self.aggregate
    }

    pub fn authorizes_durable_state_transition(&self) -> bool {
        self.authorizes_durable_state_transition
    }
}

pub struct BoltV3KillSwitchFlattenOutcomeAggregator;

impl BoltV3KillSwitchFlattenOutcomeAggregator {
    pub fn summarize(
        outcomes: &[BoltV3KillSwitchFlattenAttemptOutcome],
    ) -> BoltV3KillSwitchFlattenOutcomeSummary {
        let aggregate = if outcomes.iter().any(|outcome| {
            matches!(
                outcome,
                BoltV3KillSwitchFlattenAttemptOutcome::SubmitRejected
            )
        }) {
            BoltV3KillSwitchFlattenAggregateOutcome::SubmitRejectedManualIntervention
        } else if outcomes.iter().any(|outcome| {
            matches!(
                outcome,
                BoltV3KillSwitchFlattenAttemptOutcome::StalePositionProof
                    | BoltV3KillSwitchFlattenAttemptOutcome::UnsupportedInstrument
                    | BoltV3KillSwitchFlattenAttemptOutcome::ThinBookNoFillabilityProof
            )
        }) {
            BoltV3KillSwitchFlattenAggregateOutcome::FailedManualIntervention
        } else if outcomes.iter().any(|outcome| {
            matches!(
                outcome,
                BoltV3KillSwitchFlattenAttemptOutcome::ResidualPositionRemains
                    | BoltV3KillSwitchFlattenAttemptOutcome::PartialFill
            )
        }) {
            BoltV3KillSwitchFlattenAggregateOutcome::ResidualPositionRemains
        } else if outcomes.iter().any(|outcome| {
            matches!(
                outcome,
                BoltV3KillSwitchFlattenAttemptOutcome::SubmitPlanned
                    | BoltV3KillSwitchFlattenAttemptOutcome::SubmitAccepted
            )
        }) {
            BoltV3KillSwitchFlattenAggregateOutcome::OutstandingFlattenSubmit
        } else {
            BoltV3KillSwitchFlattenAggregateOutcome::AllFlat
        };

        BoltV3KillSwitchFlattenOutcomeSummary {
            aggregate,
            authorizes_durable_state_transition: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoltV3KillSwitchFlattenResult {
    AllFlat,
    ResidualPositionRemains,
    OutstandingFlattenSubmit,
    SubmitRejectedManualIntervention,
    FailedManualIntervention,
}

impl From<BoltV3KillSwitchFlattenAggregateOutcome> for BoltV3KillSwitchFlattenResult {
    fn from(value: BoltV3KillSwitchFlattenAggregateOutcome) -> Self {
        match value {
            BoltV3KillSwitchFlattenAggregateOutcome::AllFlat => Self::AllFlat,
            BoltV3KillSwitchFlattenAggregateOutcome::ResidualPositionRemains => {
                Self::ResidualPositionRemains
            }
            BoltV3KillSwitchFlattenAggregateOutcome::OutstandingFlattenSubmit => {
                Self::OutstandingFlattenSubmit
            }
            BoltV3KillSwitchFlattenAggregateOutcome::SubmitRejectedManualIntervention => {
                Self::SubmitRejectedManualIntervention
            }
            BoltV3KillSwitchFlattenAggregateOutcome::FailedManualIntervention => {
                Self::FailedManualIntervention
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct BoltV3KillSwitchFlattenOutcomeEvidence {
    halt_id: String,
    action_id: String,
    account_id: AccountId,
    instrument_id: InstrumentId,
    strategy_id: StrategyId,
    position_id: PositionId,
    outcome: BoltV3KillSwitchFlattenAttemptOutcome,
}

impl BoltV3KillSwitchFlattenOutcomeEvidence {
    pub fn from_command(
        command: &BoltV3KillSwitchFlattenCommand,
        outcome: BoltV3KillSwitchFlattenAttemptOutcome,
    ) -> Self {
        Self {
            halt_id: command.halt_id.clone(),
            action_id: command.action_id.clone(),
            account_id: command.account_id,
            instrument_id: command.instrument_id,
            strategy_id: command.strategy_id,
            position_id: command.position_id,
            outcome,
        }
    }

    fn identity(&self) -> BoltV3KillSwitchFlattenOutcomeIdentity {
        (
            self.halt_id.clone(),
            self.action_id.clone(),
            self.account_id,
            self.instrument_id,
            self.strategy_id,
            self.position_id,
        )
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct BoltV3KillSwitchFlattenOutcomeAggregation {
    result: BoltV3KillSwitchFlattenResult,
}

impl BoltV3KillSwitchFlattenOutcomeAggregation {
    pub fn from_plan_outcomes(
        plan: &BoltV3KillSwitchFlattenPlan,
        outcomes: Vec<BoltV3KillSwitchFlattenOutcomeEvidence>,
    ) -> Result<Self, BoltV3KillSwitchFlattenError> {
        let expected_identities = plan
            .commands()
            .iter()
            .map(|command| (flatten_outcome_identity(command), ()))
            .collect::<BTreeMap<_, _>>();

        let mut outcomes_by_identity = BTreeMap::new();
        for outcome in outcomes {
            let identity = outcome.identity();
            if !expected_identities.contains_key(&identity) {
                continue;
            }
            outcomes_by_identity
                .entry(identity)
                .and_modify(|existing| {
                    *existing = worse_flatten_outcome(*existing, outcome.outcome);
                })
                .or_insert(outcome.outcome);
        }

        let mut aggregate = BoltV3KillSwitchFlattenAggregateOutcome::AllFlat;
        for command in plan.commands() {
            let Some(outcome) = outcomes_by_identity.get(&flatten_outcome_identity(command)) else {
                aggregate = worse_flatten_aggregate(
                    aggregate,
                    BoltV3KillSwitchFlattenAggregateOutcome::OutstandingFlattenSubmit,
                );
                continue;
            };
            aggregate = worse_flatten_aggregate(aggregate, aggregate_for_outcome(*outcome));
        }
        Ok(Self {
            result: aggregate.into(),
        })
    }

    pub fn result(&self) -> BoltV3KillSwitchFlattenResult {
        self.result
    }
}

type BoltV3KillSwitchFlattenOutcomeIdentity = (
    String,
    String,
    AccountId,
    InstrumentId,
    StrategyId,
    PositionId,
);

fn flatten_outcome_identity(
    command: &BoltV3KillSwitchFlattenCommand,
) -> BoltV3KillSwitchFlattenOutcomeIdentity {
    (
        command.halt_id().to_string(),
        command.action_id().to_string(),
        command.account_id(),
        command.instrument_id(),
        command.strategy_id(),
        command.position_id(),
    )
}

fn worse_flatten_outcome(
    current: BoltV3KillSwitchFlattenAttemptOutcome,
    next: BoltV3KillSwitchFlattenAttemptOutcome,
) -> BoltV3KillSwitchFlattenAttemptOutcome {
    if aggregate_for_outcome(next) > aggregate_for_outcome(current) {
        next
    } else {
        current
    }
}

fn aggregate_for_outcome(
    outcome: BoltV3KillSwitchFlattenAttemptOutcome,
) -> BoltV3KillSwitchFlattenAggregateOutcome {
    BoltV3KillSwitchFlattenOutcomeAggregator::summarize(&[outcome]).aggregate()
}

fn worse_flatten_aggregate(
    current: BoltV3KillSwitchFlattenAggregateOutcome,
    next: BoltV3KillSwitchFlattenAggregateOutcome,
) -> BoltV3KillSwitchFlattenAggregateOutcome {
    if next > current { next } else { current }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BoltV3KillSwitchFlattenRetryPolicy {
    retry_max_attempts: u32,
    retry_timeout_ms: u64,
    retry_backoff_ms: u64,
}

impl BoltV3KillSwitchFlattenRetryPolicy {
    pub fn new(
        retry_max_attempts: u32,
        retry_timeout_ms: u64,
        retry_backoff_ms: u64,
    ) -> Result<Self, BoltV3KillSwitchFlattenError> {
        if retry_max_attempts == 0 || retry_timeout_ms == 0 || retry_backoff_ms == 0 {
            return Err(BoltV3KillSwitchFlattenError::InvalidRetryPolicy);
        }
        Ok(Self {
            retry_max_attempts,
            retry_timeout_ms,
            retry_backoff_ms,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BoltV3KillSwitchFlattenRetryContext {
    pub attempts: u32,
    pub elapsed_ms: u64,
    pub nt_trading_state: TradingState,
    pub live_forced_reduction_order_count: u32,
    pub max_live_forced_reduction_order_count: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoltV3KillSwitchFlattenRetryDecision {
    RetryAllowed { backoff_ms: u64 },
    ExhaustedManualIntervention,
    TimedOutManualIntervention,
    RouteNoLongerReducingManualIntervention,
    ForcedReductionCapUnavailable,
}

pub struct BoltV3KillSwitchFlattenRetrySupervisor;

impl BoltV3KillSwitchFlattenRetrySupervisor {
    pub fn decide(
        policy: BoltV3KillSwitchFlattenRetryPolicy,
        context: BoltV3KillSwitchFlattenRetryContext,
    ) -> BoltV3KillSwitchFlattenRetryDecision {
        if context.nt_trading_state != TradingState::Reducing {
            return BoltV3KillSwitchFlattenRetryDecision::RouteNoLongerReducingManualIntervention;
        }
        if context.live_forced_reduction_order_count
            >= context.max_live_forced_reduction_order_count
        {
            return BoltV3KillSwitchFlattenRetryDecision::ForcedReductionCapUnavailable;
        }
        if context.attempts >= policy.retry_max_attempts {
            return BoltV3KillSwitchFlattenRetryDecision::ExhaustedManualIntervention;
        }
        if context.elapsed_ms >= policy.retry_timeout_ms {
            return BoltV3KillSwitchFlattenRetryDecision::TimedOutManualIntervention;
        }
        BoltV3KillSwitchFlattenRetryDecision::RetryAllowed {
            backoff_ms: policy.retry_backoff_ms,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoltV3KillSwitchFlattenError {
    ConflictingPositionProof,
    ForcedReductionProofMismatch,
    InconsistentPositionProof,
    InvalidConfigSha256,
    InvalidOrderTemplate,
    InvalidPolicySha256,
    InvalidRetryPolicy,
    InvalidAttemptOutcome,
    MissingActionId,
    KillSwitchStateNotFlattening,
    MissingCandidates,
    MissingPositionProof,
    MissingSourceTimestamp,
    NonPositiveSourceFreshness,
    NtTradingStateNotReducing,
    OrderTemplateNotReduceOnly,
    OrderTemplateUsesQuoteQuantity,
    StaleSourceTimestamp,
    UnsupportedRouteProof,
}
