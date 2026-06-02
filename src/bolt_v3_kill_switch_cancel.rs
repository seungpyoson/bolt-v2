use crate::bolt_v3_kill_switch::KillSwitchState;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

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
    account_id: String,
    instrument_id: String,
    strategy_id: String,
    client_order_id: String,
    source_timestamp_unix_nanos: u64,
}

impl BoltV3KillSwitchCancelCandidate {
    pub fn new(
        surface: BoltV3KillSwitchOutstandingOrderRiskSurface,
        account_id: impl Into<String>,
        instrument_id: impl Into<String>,
        strategy_id: impl Into<String>,
        client_order_id: impl Into<String>,
        source_timestamp_unix_nanos: u64,
    ) -> Result<Self, BoltV3KillSwitchCancelError> {
        let account_id = account_id.into().trim().to_string();
        let instrument_id = instrument_id.into().trim().to_string();
        let strategy_id = strategy_id.into().trim().to_string();
        let client_order_id = client_order_id.into().trim().to_string();

        if account_id.is_empty() {
            return Err(BoltV3KillSwitchCancelError::InvalidAccountId);
        }
        if instrument_id.is_empty() {
            return Err(BoltV3KillSwitchCancelError::InvalidInstrumentId);
        }
        if strategy_id.is_empty() {
            return Err(BoltV3KillSwitchCancelError::InvalidStrategyId);
        }
        if client_order_id.is_empty() {
            return Err(BoltV3KillSwitchCancelError::InvalidClientOrderId);
        }
        if source_timestamp_unix_nanos == 0 {
            return Err(BoltV3KillSwitchCancelError::MissingSourceTimestamp);
        }

        Ok(Self {
            surface,
            account_id,
            instrument_id,
            strategy_id,
            client_order_id,
            source_timestamp_unix_nanos,
        })
    }

    pub fn surface(&self) -> BoltV3KillSwitchOutstandingOrderRiskSurface {
        self.surface
    }

    pub fn account_id(&self) -> &str {
        &self.account_id
    }

    pub fn instrument_id(&self) -> &str {
        &self.instrument_id
    }

    pub fn strategy_id(&self) -> &str {
        &self.strategy_id
    }

    pub fn client_order_id(&self) -> &str {
        &self.client_order_id
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
                candidate.account_id.clone(),
                candidate.instrument_id.clone(),
                candidate.strategy_id.clone(),
                candidate.client_order_id.clone(),
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
    max_source_age_unix_nanos: Option<u64>,
}

impl BoltV3KillSwitchCancelPolicy {
    pub fn new(
        mandatory_surfaces: impl IntoIterator<Item = BoltV3KillSwitchOutstandingOrderRiskSurface>,
    ) -> Result<Self, BoltV3KillSwitchCancelError> {
        let mandatory_surfaces = mandatory_surfaces.into_iter().collect::<BTreeSet<_>>();
        if mandatory_surfaces.is_empty() {
            return Err(BoltV3KillSwitchCancelError::MissingMandatorySurfacePolicy);
        }
        Ok(Self {
            mandatory_surfaces,
            max_source_age_unix_nanos: None,
        })
    }

    pub fn with_source_freshness(
        mandatory_surfaces: impl IntoIterator<Item = BoltV3KillSwitchOutstandingOrderRiskSurface>,
        max_source_age_unix_nanos: u64,
    ) -> Result<Self, BoltV3KillSwitchCancelError> {
        if max_source_age_unix_nanos == 0 {
            return Err(BoltV3KillSwitchCancelError::InvalidSourceFreshness);
        }
        let mut policy = Self::new(mandatory_surfaces)?;
        policy.max_source_age_unix_nanos = Some(max_source_age_unix_nanos);
        Ok(policy)
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
    account_ids: Vec<String>,
    instrument_ids: Vec<String>,
    strategy_ids: Vec<String>,
}

impl BoltV3KillSwitchCancelScope {
    pub fn new(
        account_ids: Vec<String>,
        instrument_ids: Vec<String>,
        strategy_ids: Vec<String>,
    ) -> Result<Self, BoltV3KillSwitchCancelError> {
        let account_ids = trim_required_values(account_ids)?;
        let instrument_ids = trim_required_values(instrument_ids)?;
        let strategy_ids = trim_required_values(strategy_ids)?;
        Ok(Self {
            account_ids,
            instrument_ids,
            strategy_ids,
        })
    }

    pub fn account_ids(&self) -> &[String] {
        &self.account_ids
    }

    pub fn instrument_ids(&self) -> &[String] {
        &self.instrument_ids
    }

    pub fn strategy_ids(&self) -> &[String] {
        &self.strategy_ids
    }
}

fn trim_required_values(values: Vec<String>) -> Result<Vec<String>, BoltV3KillSwitchCancelError> {
    if values.is_empty() {
        return Err(BoltV3KillSwitchCancelError::InvalidScope);
    }
    values
        .into_iter()
        .map(|value| {
            let value = value.trim().to_string();
            if value.is_empty() {
                Err(BoltV3KillSwitchCancelError::InvalidScope)
            } else {
                Ok(value)
            }
        })
        .collect()
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
        validate_source_freshness(&request)?;
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
                account_id: candidate.account_id.clone(),
                instrument_id: candidate.instrument_id.clone(),
                strategy_id: candidate.strategy_id.clone(),
                client_order_id: candidate.client_order_id.clone(),
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

fn validate_source_freshness(
    request: &BoltV3KillSwitchCancelPlanRequest,
) -> Result<(), BoltV3KillSwitchCancelError> {
    let Some(max_source_age_unix_nanos) = request.policy.max_source_age_unix_nanos else {
        return Ok(());
    };
    if source_timestamp_is_stale(
        request.source_timestamp_unix_nanos,
        request.observed_at_unix_nanos,
        max_source_age_unix_nanos,
    ) {
        return Err(BoltV3KillSwitchCancelError::StaleSourceTimestamp);
    }
    if request.snapshot.candidates().iter().any(|candidate| {
        source_timestamp_is_stale(
            candidate.source_timestamp_unix_nanos(),
            request.observed_at_unix_nanos,
            max_source_age_unix_nanos,
        )
    }) {
        return Err(BoltV3KillSwitchCancelError::StaleSourceTimestamp);
    }
    Ok(())
}

fn source_timestamp_is_stale(
    source_timestamp_unix_nanos: u64,
    observed_at_unix_nanos: u64,
    max_source_age_unix_nanos: u64,
) -> bool {
    source_timestamp_unix_nanos > observed_at_unix_nanos
        || observed_at_unix_nanos - source_timestamp_unix_nanos > max_source_age_unix_nanos
}

fn is_sha256_hex_digest(value: &str) -> bool {
    let expected_len = hex::encode(Sha256::digest([])).len();
    value.len() == expected_len && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoltV3KillSwitchCancelCommand {
    halt_id: String,
    action_id: String,
    config_sha256: String,
    policy_sha256: String,
    source_timestamp_unix_nanos: u64,
    account_id: String,
    instrument_id: String,
    strategy_id: String,
    client_order_id: String,
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

    pub fn account_id(&self) -> &str {
        &self.account_id
    }

    pub fn instrument_id(&self) -> &str {
        &self.instrument_id
    }

    pub fn strategy_id(&self) -> &str {
        &self.strategy_id
    }

    pub fn client_order_id(&self) -> &str {
        &self.client_order_id
    }

    pub fn surface(&self) -> BoltV3KillSwitchOutstandingOrderRiskSurface {
        self.surface
    }

    pub fn route_kind(&self) -> BoltV3KillSwitchCancelRouteKind {
        self.route_kind
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
    InvalidSourceFreshness,
    StaleSourceTimestamp,
    FailedManualInterventionRequired,
    KillSwitchStateNotCancelling,
}
