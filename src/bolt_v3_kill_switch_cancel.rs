use crate::bolt_v3_kill_switch::KillSwitchState;
use crate::bolt_v3_numeric::is_sha256_hex_digest;
use nautilus_model::{
    enums::OrderStatus,
    identifiers::{AccountId, ClientOrderId, InstrumentId, StrategyId},
};
use std::collections::{BTreeMap, BTreeSet};

type BoltV3KillSwitchCancelOrderIdentity = (AccountId, InstrumentId, StrategyId, ClientOrderId);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum BoltV3KillSwitchOutstandingOrderRiskSurface {
    Open,
    Inflight,
    PendingCancel,
    Emulated,
    AlgorithmManaged,
    Contingent,
    AcceptedButNotTerminal,
}

const MANDATORY_OUTSTANDING_ORDER_RISK_SURFACES: &[BoltV3KillSwitchOutstandingOrderRiskSurface] = &[
    BoltV3KillSwitchOutstandingOrderRiskSurface::Open,
    BoltV3KillSwitchOutstandingOrderRiskSurface::Inflight,
    BoltV3KillSwitchOutstandingOrderRiskSurface::PendingCancel,
    BoltV3KillSwitchOutstandingOrderRiskSurface::Emulated,
    BoltV3KillSwitchOutstandingOrderRiskSurface::AlgorithmManaged,
    BoltV3KillSwitchOutstandingOrderRiskSurface::Contingent,
    BoltV3KillSwitchOutstandingOrderRiskSurface::AcceptedButNotTerminal,
];

impl BoltV3KillSwitchOutstandingOrderRiskSurface {
    pub fn mandatory_surfaces() -> &'static [Self] {
        MANDATORY_OUTSTANDING_ORDER_RISK_SURFACES
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoltV3KillSwitchCancelCandidate {
    surface: BoltV3KillSwitchOutstandingOrderRiskSurface,
    account_id: AccountId,
    instrument_id: InstrumentId,
    strategy_id: StrategyId,
    client_order_id: ClientOrderId,
    order_status: OrderStatus,
    source_timestamp_unix_nanos: u64,
}

impl BoltV3KillSwitchCancelCandidate {
    pub fn new(
        surface: BoltV3KillSwitchOutstandingOrderRiskSurface,
        account_id: impl Into<String>,
        instrument_id: impl Into<String>,
        strategy_id: impl Into<String>,
        client_order_id: impl Into<String>,
        order_status: OrderStatus,
        source_timestamp_unix_nanos: u64,
    ) -> Result<Self, BoltV3KillSwitchCancelError> {
        let account_id = account_id.into().trim().to_string();
        let instrument_id = instrument_id.into().trim().to_string();
        let strategy_id = strategy_id.into().trim().to_string();
        let client_order_id = client_order_id.into().trim().to_string();

        let account_id = AccountId::new_checked(&account_id)
            .map_err(|_| BoltV3KillSwitchCancelError::InvalidAccountId)?;
        let instrument_id = InstrumentId::from_as_ref(&instrument_id)
            .map_err(|_| BoltV3KillSwitchCancelError::InvalidInstrumentId)?;
        let strategy_id = StrategyId::new_checked(&strategy_id)
            .map_err(|_| BoltV3KillSwitchCancelError::InvalidStrategyId)?;
        let client_order_id = ClientOrderId::new_checked(&client_order_id)
            .map_err(|_| BoltV3KillSwitchCancelError::InvalidClientOrderId)?;
        Self::from_nt_order_state(
            surface,
            account_id,
            instrument_id,
            strategy_id,
            client_order_id,
            order_status,
            source_timestamp_unix_nanos,
        )
    }

    pub fn from_nt_order_state(
        surface: BoltV3KillSwitchOutstandingOrderRiskSurface,
        account_id: AccountId,
        instrument_id: InstrumentId,
        strategy_id: StrategyId,
        client_order_id: ClientOrderId,
        order_status: OrderStatus,
        source_timestamp_unix_nanos: u64,
    ) -> Result<Self, BoltV3KillSwitchCancelError> {
        if source_timestamp_unix_nanos == 0 {
            return Err(BoltV3KillSwitchCancelError::MissingSourceTimestamp);
        }

        Ok(Self {
            surface,
            account_id,
            instrument_id,
            strategy_id,
            client_order_id,
            order_status,
            source_timestamp_unix_nanos,
        })
    }

    pub fn surface(&self) -> BoltV3KillSwitchOutstandingOrderRiskSurface {
        self.surface
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

    pub fn client_order_id(&self) -> ClientOrderId {
        self.client_order_id
    }

    pub fn order_status(&self) -> OrderStatus {
        self.order_status
    }

    pub fn source_timestamp_unix_nanos(&self) -> u64 {
        self.source_timestamp_unix_nanos
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoltV3KillSwitchCancelSnapshot {
    candidates: Vec<BoltV3KillSwitchCancelCandidate>,
    observed_surfaces: BTreeSet<BoltV3KillSwitchOutstandingOrderRiskSurface>,
}

impl BoltV3KillSwitchCancelSnapshot {
    pub fn new(
        candidates: Vec<BoltV3KillSwitchCancelCandidate>,
    ) -> Result<Self, BoltV3KillSwitchCancelError> {
        if candidates.is_empty() {
            return Err(BoltV3KillSwitchCancelError::MissingCandidates);
        }

        let mut scoped_candidates = BTreeMap::new();
        let mut observed_surfaces = BTreeSet::new();
        for candidate in candidates {
            observed_surfaces.insert(candidate.surface());
            let scoped_order_identity = (
                candidate.account_id,
                candidate.instrument_id,
                candidate.strategy_id,
                candidate.client_order_id,
            );
            scoped_candidates
                .entry(scoped_order_identity)
                .or_insert(candidate);
        }

        Ok(Self {
            candidates: scoped_candidates.into_values().collect(),
            observed_surfaces,
        })
    }

    pub fn candidates(&self) -> &[BoltV3KillSwitchCancelCandidate] {
        &self.candidates
    }

    pub fn has_outstanding_risk(&self) -> bool {
        !self.candidates.is_empty()
    }

    pub fn missing_mandatory_surfaces(
        &self,
        mandatory_surfaces: &[BoltV3KillSwitchOutstandingOrderRiskSurface],
    ) -> Vec<BoltV3KillSwitchOutstandingOrderRiskSurface> {
        mandatory_surfaces
            .iter()
            .copied()
            .filter(|surface| !self.observed_surfaces.contains(surface))
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoltV3KillSwitchCancelPolicy {
    mandatory_surfaces: BTreeSet<BoltV3KillSwitchOutstandingOrderRiskSurface>,
}

impl BoltV3KillSwitchCancelPolicy {
    pub fn new(
        mandatory_surfaces: impl IntoIterator<Item = BoltV3KillSwitchOutstandingOrderRiskSurface>,
    ) -> Result<Self, BoltV3KillSwitchCancelError> {
        let mandatory_surfaces = mandatory_surfaces.into_iter().collect::<BTreeSet<_>>();
        if mandatory_surfaces.is_empty() {
            return Err(BoltV3KillSwitchCancelError::MissingMandatorySurfacePolicy);
        }
        Ok(Self { mandatory_surfaces })
    }

    pub fn validate_snapshot(
        &self,
        snapshot: &BoltV3KillSwitchCancelSnapshot,
    ) -> Result<(), BoltV3KillSwitchCancelError> {
        let mandatory_surfaces = self.mandatory_surfaces.iter().copied().collect::<Vec<_>>();
        if snapshot
            .missing_mandatory_surfaces(&mandatory_surfaces)
            .is_empty()
        {
            Ok(())
        } else {
            Err(BoltV3KillSwitchCancelError::MissingMandatorySurfaceProof)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoltV3KillSwitchCancelDecisionMode {
    DryRunProofOnly,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoltV3KillSwitchCancelPlanRequest {
    pub kill_switch_state: KillSwitchState,
    pub action_id: String,
    pub config_sha256: String,
    pub policy_sha256: String,
    pub source_timestamp_unix_nanos: u64,
    pub observed_at_unix_nanos: u64,
    pub scope: BoltV3KillSwitchCancelScope,
    pub route_proof: Option<BoltV3KillSwitchCancelRouteProof>,
    pub policy: BoltV3KillSwitchCancelPolicy,
    pub snapshot: BoltV3KillSwitchCancelSnapshot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoltV3KillSwitchCancelRouteKind {
    PerStrategyActionPort,
    LiveNodeCommandRouter,
    Unsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BoltV3KillSwitchCancelRouteProof {
    route_kind: BoltV3KillSwitchCancelRouteKind,
}

impl BoltV3KillSwitchCancelRouteProof {
    pub fn new(route_kind: BoltV3KillSwitchCancelRouteKind) -> Self {
        Self { route_kind }
    }

    pub fn route_kind(&self) -> BoltV3KillSwitchCancelRouteKind {
        self.route_kind
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoltV3KillSwitchCancelScope {
    account_ids: Vec<AccountId>,
    instrument_ids: Vec<InstrumentId>,
    strategy_ids: Vec<StrategyId>,
}

impl BoltV3KillSwitchCancelScope {
    pub fn new(
        account_ids: Vec<AccountId>,
        instrument_ids: Vec<InstrumentId>,
        strategy_ids: Vec<StrategyId>,
    ) -> Result<Self, BoltV3KillSwitchCancelError> {
        if account_ids.is_empty() || instrument_ids.is_empty() || strategy_ids.is_empty() {
            return Err(BoltV3KillSwitchCancelError::InvalidScope);
        }
        Ok(Self {
            account_ids,
            instrument_ids,
            strategy_ids,
        })
    }

    pub fn account_ids(&self) -> &[AccountId] {
        &self.account_ids
    }

    pub fn instrument_ids(&self) -> &[InstrumentId] {
        &self.instrument_ids
    }

    pub fn strategy_ids(&self) -> &[StrategyId] {
        &self.strategy_ids
    }

    fn validate_snapshot(
        &self,
        snapshot: &BoltV3KillSwitchCancelSnapshot,
    ) -> Result<(), BoltV3KillSwitchCancelError> {
        if snapshot.candidates().iter().all(|candidate| {
            self.account_ids.contains(&candidate.account_id())
                && self.instrument_ids.contains(&candidate.instrument_id())
                && self.strategy_ids.contains(&candidate.strategy_id())
        }) {
            Ok(())
        } else {
            Err(BoltV3KillSwitchCancelError::OutOfScopeCancelCandidate)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoltV3KillSwitchCancelPlan {
    halt_id: String,
    decision_mode: BoltV3KillSwitchCancelDecisionMode,
    candidates: Vec<BoltV3KillSwitchCancelCandidate>,
    commands: Vec<BoltV3KillSwitchCancelCommand>,
}

impl BoltV3KillSwitchCancelPlan {
    pub fn halt_id(&self) -> &str {
        &self.halt_id
    }

    pub fn decision_mode(&self) -> BoltV3KillSwitchCancelDecisionMode {
        self.decision_mode
    }

    pub fn candidates(&self) -> &[BoltV3KillSwitchCancelCandidate] {
        &self.candidates
    }

    pub fn commands(&self) -> &[BoltV3KillSwitchCancelCommand] {
        &self.commands
    }
}

pub struct BoltV3KillSwitchCancelSupervisor;

impl BoltV3KillSwitchCancelSupervisor {
    pub fn plan_cancel(
        request: BoltV3KillSwitchCancelPlanRequest,
    ) -> Result<BoltV3KillSwitchCancelPlan, BoltV3KillSwitchCancelError> {
        let halt_id = match &request.kill_switch_state {
            KillSwitchState::Cancelling { halt_id } => halt_id.clone(),
            _ => return Err(BoltV3KillSwitchCancelError::KillSwitchStateNotCancelling),
        };
        validate_plan_metadata(&request)?;
        request.scope.validate_snapshot(&request.snapshot)?;
        request.policy.validate_snapshot(&request.snapshot)?;
        let route_kind = require_supported_route_proof(&request)?;
        let action_id = request.action_id.trim().to_string();
        let commands = request
            .snapshot
            .candidates()
            .iter()
            .map(|candidate| BoltV3KillSwitchCancelCommand {
                halt_id: halt_id.clone(),
                action_id: action_id.clone(),
                config_sha256: request.config_sha256.clone(),
                policy_sha256: request.policy_sha256.clone(),
                source_timestamp_unix_nanos: request.source_timestamp_unix_nanos,
                account_id: candidate.account_id,
                instrument_id: candidate.instrument_id,
                strategy_id: candidate.strategy_id,
                client_order_id: candidate.client_order_id,
                order_status: candidate.order_status,
                surface: candidate.surface(),
                route_kind,
            })
            .collect::<Vec<_>>();
        Ok(BoltV3KillSwitchCancelPlan {
            halt_id,
            decision_mode: BoltV3KillSwitchCancelDecisionMode::DryRunProofOnly,
            candidates: request.snapshot.candidates().to_vec(),
            commands,
        })
    }
}

fn require_supported_route_proof(
    request: &BoltV3KillSwitchCancelPlanRequest,
) -> Result<BoltV3KillSwitchCancelRouteKind, BoltV3KillSwitchCancelError> {
    match request
        .route_proof
        .map(|route_proof| route_proof.route_kind())
    {
        Some(
            route_kind @ (BoltV3KillSwitchCancelRouteKind::PerStrategyActionPort
            | BoltV3KillSwitchCancelRouteKind::LiveNodeCommandRouter),
        ) => Ok(route_kind),
        Some(BoltV3KillSwitchCancelRouteKind::Unsupported) | None => {
            Err(BoltV3KillSwitchCancelError::FailedManualInterventionRequired)
        }
    }
}

fn validate_plan_metadata(
    request: &BoltV3KillSwitchCancelPlanRequest,
) -> Result<(), BoltV3KillSwitchCancelError> {
    if request.action_id.trim().is_empty() {
        return Err(BoltV3KillSwitchCancelError::MissingActionId);
    }
    if !is_sha256_hex_digest(&request.config_sha256) {
        return Err(BoltV3KillSwitchCancelError::InvalidConfigSha256);
    }
    if !is_sha256_hex_digest(&request.policy_sha256) {
        return Err(BoltV3KillSwitchCancelError::InvalidPolicySha256);
    }
    if request.source_timestamp_unix_nanos == 0 {
        return Err(BoltV3KillSwitchCancelError::MissingSourceTimestamp);
    }
    if request.observed_at_unix_nanos == 0 {
        return Err(BoltV3KillSwitchCancelError::MissingObservationTimestamp);
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoltV3KillSwitchCancelCommand {
    halt_id: String,
    action_id: String,
    config_sha256: String,
    policy_sha256: String,
    source_timestamp_unix_nanos: u64,
    account_id: AccountId,
    instrument_id: InstrumentId,
    strategy_id: StrategyId,
    client_order_id: ClientOrderId,
    order_status: OrderStatus,
    surface: BoltV3KillSwitchOutstandingOrderRiskSurface,
    route_kind: BoltV3KillSwitchCancelRouteKind,
}

impl BoltV3KillSwitchCancelCommand {
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

    pub fn client_order_id(&self) -> ClientOrderId {
        self.client_order_id
    }

    pub fn order_status(&self) -> OrderStatus {
        self.order_status
    }

    pub fn surface(&self) -> BoltV3KillSwitchOutstandingOrderRiskSurface {
        self.surface
    }

    pub fn route_kind(&self) -> BoltV3KillSwitchCancelRouteKind {
        self.route_kind
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoltV3KillSwitchCancelAttemptOutcomeKind {
    CancelRequested,
    CancelAccepted,
    CancelRejected,
    PendingCancel,
    Expired,
    FilledBeforeCancel,
    TerminalBeforeCancel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BoltV3KillSwitchCancelAttemptOutcome {
    kind: BoltV3KillSwitchCancelAttemptOutcomeKind,
    order_status: OrderStatus,
}

impl BoltV3KillSwitchCancelAttemptOutcome {
    pub fn cancel_requested(
        order_status: OrderStatus,
    ) -> Result<Self, BoltV3KillSwitchCancelError> {
        Self::new(
            BoltV3KillSwitchCancelAttemptOutcomeKind::CancelRequested,
            order_status,
        )
    }

    pub fn cancel_accepted(order_status: OrderStatus) -> Result<Self, BoltV3KillSwitchCancelError> {
        Self::new(
            BoltV3KillSwitchCancelAttemptOutcomeKind::CancelAccepted,
            order_status,
        )
    }

    pub fn cancel_rejected(order_status: OrderStatus) -> Result<Self, BoltV3KillSwitchCancelError> {
        Self::new(
            BoltV3KillSwitchCancelAttemptOutcomeKind::CancelRejected,
            order_status,
        )
    }

    pub fn pending_cancel(order_status: OrderStatus) -> Result<Self, BoltV3KillSwitchCancelError> {
        Self::new(
            BoltV3KillSwitchCancelAttemptOutcomeKind::PendingCancel,
            order_status,
        )
    }

    pub fn expired(order_status: OrderStatus) -> Result<Self, BoltV3KillSwitchCancelError> {
        Self::new(
            BoltV3KillSwitchCancelAttemptOutcomeKind::Expired,
            order_status,
        )
    }

    pub fn filled_before_cancel(
        order_status: OrderStatus,
    ) -> Result<Self, BoltV3KillSwitchCancelError> {
        Self::new(
            BoltV3KillSwitchCancelAttemptOutcomeKind::FilledBeforeCancel,
            order_status,
        )
    }

    pub fn terminal_before_cancel(
        order_status: OrderStatus,
    ) -> Result<Self, BoltV3KillSwitchCancelError> {
        Self::new(
            BoltV3KillSwitchCancelAttemptOutcomeKind::TerminalBeforeCancel,
            order_status,
        )
    }

    fn new(
        kind: BoltV3KillSwitchCancelAttemptOutcomeKind,
        order_status: OrderStatus,
    ) -> Result<Self, BoltV3KillSwitchCancelError> {
        validate_outcome_status(kind, order_status)?;
        Ok(Self { kind, order_status })
    }

    pub fn kind(&self) -> BoltV3KillSwitchCancelAttemptOutcomeKind {
        self.kind
    }

    pub fn order_status(&self) -> OrderStatus {
        self.order_status
    }
}

fn validate_outcome_status(
    kind: BoltV3KillSwitchCancelAttemptOutcomeKind,
    order_status: OrderStatus,
) -> Result<(), BoltV3KillSwitchCancelError> {
    match kind {
        BoltV3KillSwitchCancelAttemptOutcomeKind::PendingCancel => {
            if order_status != OrderStatus::PendingCancel {
                return Err(BoltV3KillSwitchCancelError::InvalidOutcomeOrderStatus);
            }
        }
        BoltV3KillSwitchCancelAttemptOutcomeKind::Expired => {
            if order_status != OrderStatus::Expired {
                return Err(BoltV3KillSwitchCancelError::InvalidOutcomeOrderStatus);
            }
        }
        BoltV3KillSwitchCancelAttemptOutcomeKind::FilledBeforeCancel => {
            if order_status != OrderStatus::Filled {
                return Err(BoltV3KillSwitchCancelError::InvalidOutcomeOrderStatus);
            }
        }
        BoltV3KillSwitchCancelAttemptOutcomeKind::TerminalBeforeCancel => {
            if !order_status.is_closed()
                || matches!(order_status, OrderStatus::Expired | OrderStatus::Filled)
            {
                return Err(BoltV3KillSwitchCancelError::InvalidOutcomeOrderStatus);
            }
        }
        BoltV3KillSwitchCancelAttemptOutcomeKind::CancelRequested
        | BoltV3KillSwitchCancelAttemptOutcomeKind::CancelAccepted
        | BoltV3KillSwitchCancelAttemptOutcomeKind::CancelRejected => {
            if order_status.is_closed() {
                return Err(BoltV3KillSwitchCancelError::InvalidOutcomeOrderStatus);
            }
        }
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoltV3KillSwitchCancelOutcomeEvidence {
    account_id: AccountId,
    instrument_id: InstrumentId,
    strategy_id: StrategyId,
    client_order_id: ClientOrderId,
    outcome: BoltV3KillSwitchCancelAttemptOutcome,
}

impl BoltV3KillSwitchCancelOutcomeEvidence {
    pub fn from_candidate(
        candidate: &BoltV3KillSwitchCancelCandidate,
        outcome: BoltV3KillSwitchCancelAttemptOutcome,
    ) -> Self {
        Self {
            account_id: candidate.account_id(),
            instrument_id: candidate.instrument_id(),
            strategy_id: candidate.strategy_id(),
            client_order_id: candidate.client_order_id(),
            outcome,
        }
    }

    fn identity(&self) -> BoltV3KillSwitchCancelOrderIdentity {
        (
            self.account_id,
            self.instrument_id,
            self.strategy_id,
            self.client_order_id,
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoltV3KillSwitchCancelAggregateResult {
    AllTerminal,
    OutstandingRiskRemains,
    RequiresPositionReconciliation,
    FailedManualIntervention,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoltV3KillSwitchCancelOutcomeAggregation {
    result: BoltV3KillSwitchCancelAggregateResult,
}

impl BoltV3KillSwitchCancelOutcomeAggregation {
    pub fn from_snapshot_outcomes(
        snapshot: &BoltV3KillSwitchCancelSnapshot,
        outcomes: Vec<BoltV3KillSwitchCancelOutcomeEvidence>,
    ) -> Result<Self, BoltV3KillSwitchCancelError> {
        let expected_identities = snapshot
            .candidates()
            .iter()
            .map(|candidate| (cancel_candidate_identity(candidate), ()))
            .collect::<BTreeMap<_, _>>();

        let mut outcomes_by_identity = BTreeMap::new();
        for outcome in outcomes {
            let identity = outcome.identity();
            if !expected_identities.contains_key(&identity) {
                return Err(BoltV3KillSwitchCancelError::UnknownOutcomeCandidate);
            }
            outcomes_by_identity
                .entry(identity)
                .and_modify(|existing| {
                    *existing = worse_outcome(*existing, outcome.outcome);
                })
                .or_insert(outcome.outcome);
        }

        let mut result = BoltV3KillSwitchCancelAggregateResult::AllTerminal;
        for candidate in snapshot.candidates() {
            let identity = cancel_candidate_identity(candidate);
            let Some(outcome) = outcomes_by_identity.get(&identity) else {
                result = merge_aggregate_result(
                    result,
                    BoltV3KillSwitchCancelAggregateResult::OutstandingRiskRemains,
                );
                continue;
            };
            result = merge_aggregate_result(result, result_for_outcome(*outcome));
        }

        Ok(Self { result })
    }

    pub fn result(&self) -> BoltV3KillSwitchCancelAggregateResult {
        self.result
    }
}

fn cancel_candidate_identity(
    candidate: &BoltV3KillSwitchCancelCandidate,
) -> BoltV3KillSwitchCancelOrderIdentity {
    (
        candidate.account_id(),
        candidate.instrument_id(),
        candidate.strategy_id(),
        candidate.client_order_id(),
    )
}

fn result_for_outcome(
    outcome: BoltV3KillSwitchCancelAttemptOutcome,
) -> BoltV3KillSwitchCancelAggregateResult {
    match outcome.kind() {
        BoltV3KillSwitchCancelAttemptOutcomeKind::CancelRejected => {
            BoltV3KillSwitchCancelAggregateResult::FailedManualIntervention
        }
        BoltV3KillSwitchCancelAttemptOutcomeKind::FilledBeforeCancel => {
            BoltV3KillSwitchCancelAggregateResult::RequiresPositionReconciliation
        }
        BoltV3KillSwitchCancelAttemptOutcomeKind::CancelRequested
        | BoltV3KillSwitchCancelAttemptOutcomeKind::CancelAccepted
        | BoltV3KillSwitchCancelAttemptOutcomeKind::PendingCancel => {
            BoltV3KillSwitchCancelAggregateResult::OutstandingRiskRemains
        }
        BoltV3KillSwitchCancelAttemptOutcomeKind::Expired
        | BoltV3KillSwitchCancelAttemptOutcomeKind::TerminalBeforeCancel => {
            BoltV3KillSwitchCancelAggregateResult::AllTerminal
        }
    }
}

fn merge_aggregate_result(
    current: BoltV3KillSwitchCancelAggregateResult,
    next: BoltV3KillSwitchCancelAggregateResult,
) -> BoltV3KillSwitchCancelAggregateResult {
    use BoltV3KillSwitchCancelAggregateResult::{
        AllTerminal, FailedManualIntervention, OutstandingRiskRemains,
        RequiresPositionReconciliation,
    };
    match (current, next) {
        (FailedManualIntervention, _) | (_, FailedManualIntervention) => FailedManualIntervention,
        (RequiresPositionReconciliation, _) | (_, RequiresPositionReconciliation) => {
            RequiresPositionReconciliation
        }
        (OutstandingRiskRemains, _) | (_, OutstandingRiskRemains) => OutstandingRiskRemains,
        (AllTerminal, AllTerminal) => AllTerminal,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoltV3KillSwitchCancelError {
    MissingActionId,
    InvalidConfigSha256,
    InvalidPolicySha256,
    InvalidScope,
    InvalidAccountId,
    InvalidInstrumentId,
    InvalidStrategyId,
    InvalidClientOrderId,
    MissingSourceTimestamp,
    MissingObservationTimestamp,
    MissingCandidates,
    MissingMandatorySurfacePolicy,
    MissingMandatorySurfaceProof,
    FailedManualInterventionRequired,
    KillSwitchStateNotCancelling,
    InvalidOutcomeOrderStatus,
    UnknownOutcomeCandidate,
    OutOfScopeCancelCandidate,
}

fn worse_outcome(
    current: BoltV3KillSwitchCancelAttemptOutcome,
    next: BoltV3KillSwitchCancelAttemptOutcome,
) -> BoltV3KillSwitchCancelAttemptOutcome {
    let current_result = result_for_outcome(current);
    let next_result = result_for_outcome(next);
    if next_result == merge_aggregate_result(current_result, next_result) {
        next
    } else {
        current
    }
}
