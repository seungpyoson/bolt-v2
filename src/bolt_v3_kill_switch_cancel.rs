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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoltV3KillSwitchCancelError {
    InvalidAccountId,
    InvalidInstrumentId,
    InvalidStrategyId,
    InvalidClientOrderId,
    MissingSourceTimestamp,
    MissingCandidates,
}
