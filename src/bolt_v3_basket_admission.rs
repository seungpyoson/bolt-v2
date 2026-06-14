use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use crate::bolt_v3_decision_evidence::{
    BoltV3BasketAdmissionDecisionEvidence, BoltV3BasketAdmissionOutcome,
    BoltV3DecisionEvidenceWriter,
};
use crate::bolt_v3_outcome_group_scanner::OutcomeGroupScanEvidence;
use crate::bolt_v3_outcome_groups::{OutcomeGroup, ValidatedOutcomeGroup};
use crate::bolt_v3_submit_admission::{
    BoltV3BasketSubmitSlotClaim, BoltV3SubmitAdmissionError, BoltV3SubmitAdmissionState,
};
use rust_decimal::Decimal;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoltV3BasketAdmissionLimits {
    pub max_basket_notional: Decimal,
    pub max_open_baskets: u32,
    pub min_edge_bps: Decimal,
    pub max_scanner_evidence_age_ms: u64,
    pub max_submit_recheck_age_ms: u64,
    pub max_retry_count: u32,
}

#[derive(Debug, Clone)]
pub struct BoltV3BasketAdmissionRequest<'a> {
    pub strategy_id: &'a str,
    pub basket_id: &'a str,
    pub execution_client_id: &'a str,
    pub group: &'a OutcomeGroup,
    pub scanner_evidence: &'a OutcomeGroupScanEvidence,
    pub submit_claims: Vec<BoltV3BasketSubmitSlotClaim>,
    pub now_unix_ms: u64,
    pub submit_recheck_observed_unix_ms: u64,
    pub retry_count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BoltV3BasketAdmissionError {
    BasketNotionalCapExceeded,
    MaxOpenBasketCapExceeded,
    StaleScannerEvidence,
    StaleSubmitRecheck,
    NonPositiveCandidateCost,
    NonPositiveEdge,
    EdgeThreshold,
    MissingGroupingProof,
    MissingSettlementRules,
    RetryBudgetExceeded,
    BasketAlreadyOpen,
    BasketReservationMissing,
    SubmitAdmissionFailed(BoltV3SubmitAdmissionError),
    EvidenceWriteFailed { reason: String },
}

impl std::fmt::Display for BoltV3BasketAdmissionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BasketNotionalCapExceeded => write!(f, "basket admission notional cap exceeded"),
            Self::MaxOpenBasketCapExceeded => {
                write!(f, "basket admission open basket cap exceeded")
            }
            Self::StaleScannerEvidence => write!(f, "basket admission scanner evidence is stale"),
            Self::StaleSubmitRecheck => write!(f, "basket admission submit recheck is stale"),
            Self::NonPositiveCandidateCost => {
                write!(f, "basket admission candidate cost must be positive")
            }
            Self::NonPositiveEdge => write!(f, "basket admission edge must be positive"),
            Self::EdgeThreshold => write!(f, "basket admission edge threshold not met"),
            Self::MissingGroupingProof => write!(f, "basket admission missing grouping proof"),
            Self::MissingSettlementRules => write!(f, "basket admission missing settlement rules"),
            Self::RetryBudgetExceeded => write!(f, "basket admission retry budget exceeded"),
            Self::BasketAlreadyOpen => write!(f, "basket admission basket id is already open"),
            Self::BasketReservationMissing => {
                write!(f, "basket admission reservation does not exist")
            }
            Self::SubmitAdmissionFailed(error) => write!(f, "{error}"),
            Self::EvidenceWriteFailed { reason } => {
                write!(f, "basket admission evidence write failed: {reason}")
            }
        }
    }
}

impl std::error::Error for BoltV3BasketAdmissionError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoltV3BasketAdmissionReleaseReason {
    Terminal,
    Abort,
    Reject,
    Stuck,
}

#[derive(Debug)]
pub struct BoltV3BasketAdmissionState {
    limits: BoltV3BasketAdmissionLimits,
    inner: Mutex<BoltV3BasketAdmissionInner>,
    decision_evidence: Arc<dyn BoltV3DecisionEvidenceWriter>,
}

#[derive(Debug)]
struct BoltV3BasketAdmissionInner {
    open_baskets: BTreeMap<String, BoltV3BasketReservation>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BoltV3BasketReservation {
    strategy_id: String,
    group_id: String,
    total_notional: Decimal,
}

#[derive(Debug)]
pub struct BoltV3BasketAdmissionPermit(());

impl BoltV3BasketAdmissionState {
    pub fn new(
        decision_evidence: Arc<dyn BoltV3DecisionEvidenceWriter>,
        limits: BoltV3BasketAdmissionLimits,
    ) -> Self {
        Self {
            limits,
            inner: Mutex::new(BoltV3BasketAdmissionInner {
                open_baskets: BTreeMap::new(),
            }),
            decision_evidence,
        }
    }

    pub fn admit(
        &self,
        request: &BoltV3BasketAdmissionRequest<'_>,
        submit_admission: &BoltV3SubmitAdmissionState,
    ) -> Result<BoltV3BasketAdmissionPermit, BoltV3BasketAdmissionError> {
        if let Err(error) = self.evaluate_basket_request(request) {
            self.record_basket_decision(request, basket_outcome_from_error(&error))?;
            return Err(error);
        }
        if let Err(error) = self.reserve_open_basket(request) {
            self.record_basket_decision(request, basket_outcome_from_error(&error))?;
            return Err(error);
        }

        let evidence = basket_decision_evidence(request, BoltV3BasketAdmissionOutcome::Admitted)?;
        if let Err(error) = submit_admission.reserve_basket_submit_slots(
            request.execution_client_id,
            &request.submit_claims,
            &evidence,
        ) {
            self.release_reserved_basket_after_failed_submit(request.basket_id);
            return Err(BoltV3BasketAdmissionError::SubmitAdmissionFailed(error));
        }
        Ok(BoltV3BasketAdmissionPermit(()))
    }

    pub fn release_basket(
        &self,
        basket_id: &str,
        _reason: BoltV3BasketAdmissionReleaseReason,
    ) -> Result<(), BoltV3BasketAdmissionError> {
        let mut inner = self
            .inner
            .lock()
            .expect("basket admission state mutex should not be poisoned");
        if inner.open_baskets.remove(basket_id).is_some() {
            Ok(())
        } else {
            Err(BoltV3BasketAdmissionError::BasketReservationMissing)
        }
    }

    fn evaluate_basket_request(
        &self,
        request: &BoltV3BasketAdmissionRequest<'_>,
    ) -> Result<(), BoltV3BasketAdmissionError> {
        if request.group.grouping_proof.is_none()
            || request.scanner_evidence.grouping_proof.is_none()
        {
            return Err(BoltV3BasketAdmissionError::MissingGroupingProof);
        }
        if ValidatedOutcomeGroup::validate(request.group).is_err() {
            return Err(BoltV3BasketAdmissionError::MissingSettlementRules);
        }
        if request.retry_count > self.limits.max_retry_count {
            return Err(BoltV3BasketAdmissionError::RetryBudgetExceeded);
        }
        if request.scanner_evidence.total_adjusted_cost <= Decimal::ZERO
            || request.submit_claims.is_empty()
        {
            return Err(BoltV3BasketAdmissionError::NonPositiveCandidateCost);
        }
        if request.scanner_evidence.total_adjusted_cost > self.limits.max_basket_notional {
            return Err(BoltV3BasketAdmissionError::BasketNotionalCapExceeded);
        }
        if !request.scanner_evidence.admissible || request.scanner_evidence.block_reason.is_some() {
            return Err(BoltV3BasketAdmissionError::NonPositiveCandidateCost);
        }
        if request.scanner_evidence.absolute_edge <= Decimal::ZERO {
            return Err(BoltV3BasketAdmissionError::NonPositiveEdge);
        }
        if request.scanner_evidence.edge_bps < self.limits.min_edge_bps {
            return Err(BoltV3BasketAdmissionError::EdgeThreshold);
        }
        if request.scanner_evidence.leg_costs.iter().any(|leg| {
            request.now_unix_ms.saturating_sub(leg.observed_unix_ms)
                > self.limits.max_scanner_evidence_age_ms
        }) {
            return Err(BoltV3BasketAdmissionError::StaleScannerEvidence);
        }
        if request
            .now_unix_ms
            .saturating_sub(request.submit_recheck_observed_unix_ms)
            > self.limits.max_submit_recheck_age_ms
        {
            return Err(BoltV3BasketAdmissionError::StaleSubmitRecheck);
        }

        Ok(())
    }

    fn reserve_open_basket(
        &self,
        request: &BoltV3BasketAdmissionRequest<'_>,
    ) -> Result<(), BoltV3BasketAdmissionError> {
        let mut inner = self
            .inner
            .lock()
            .expect("basket admission state mutex should not be poisoned");
        if inner.open_baskets.contains_key(request.basket_id) {
            return Err(BoltV3BasketAdmissionError::BasketAlreadyOpen);
        }
        if u32::try_from(inner.open_baskets.len())
            .map(|open_count| open_count >= self.limits.max_open_baskets)
            .unwrap_or(true)
        {
            return Err(BoltV3BasketAdmissionError::MaxOpenBasketCapExceeded);
        }
        inner.open_baskets.insert(
            request.basket_id.to_string(),
            BoltV3BasketReservation {
                strategy_id: request.strategy_id.to_string(),
                group_id: request.group.group_id.clone(),
                total_notional: request.scanner_evidence.total_adjusted_cost,
            },
        );
        Ok(())
    }

    fn release_reserved_basket_after_failed_submit(&self, basket_id: &str) {
        let mut inner = self
            .inner
            .lock()
            .expect("basket admission state mutex should not be poisoned");
        inner.open_baskets.remove(basket_id);
    }

    fn record_basket_decision(
        &self,
        request: &BoltV3BasketAdmissionRequest<'_>,
        outcome: BoltV3BasketAdmissionOutcome,
    ) -> Result<(), BoltV3BasketAdmissionError> {
        let evidence = basket_decision_evidence(request, outcome)?;
        self.decision_evidence
            .record_basket_admission_decision(&evidence)
            .map_err(|err| BoltV3BasketAdmissionError::EvidenceWriteFailed {
                reason: format!("{err:#}"),
            })
    }
}

fn basket_decision_evidence(
    request: &BoltV3BasketAdmissionRequest<'_>,
    outcome: BoltV3BasketAdmissionOutcome,
) -> Result<BoltV3BasketAdmissionDecisionEvidence, BoltV3BasketAdmissionError> {
    let leg_order_count = u32::try_from(request.submit_claims.len()).map_err(|_| {
        BoltV3BasketAdmissionError::SubmitAdmissionFailed(
            BoltV3SubmitAdmissionError::CountCapExhausted,
        )
    })?;
    Ok(BoltV3BasketAdmissionDecisionEvidence {
        strategy_id: request.strategy_id.to_string(),
        execution_client_id: request.execution_client_id.to_string(),
        basket_id: request.basket_id.to_string(),
        group_id: request.group.group_id.clone(),
        leg_instrument_ids: request
            .submit_claims
            .iter()
            .map(|claim| claim.instrument_id.clone())
            .collect(),
        total_notional: request.scanner_evidence.total_adjusted_cost.to_string(),
        leg_order_count,
        outcome,
    })
}

fn basket_outcome_from_error(error: &BoltV3BasketAdmissionError) -> BoltV3BasketAdmissionOutcome {
    match error {
        BoltV3BasketAdmissionError::BasketNotionalCapExceeded => {
            BoltV3BasketAdmissionOutcome::RejectedBasketNotionalCapExceeded
        }
        BoltV3BasketAdmissionError::MaxOpenBasketCapExceeded => {
            BoltV3BasketAdmissionOutcome::RejectedMaxOpenBasketCapExceeded
        }
        BoltV3BasketAdmissionError::StaleScannerEvidence => {
            BoltV3BasketAdmissionOutcome::RejectedStaleScannerEvidence
        }
        BoltV3BasketAdmissionError::StaleSubmitRecheck => {
            BoltV3BasketAdmissionOutcome::RejectedStaleSubmitRecheck
        }
        BoltV3BasketAdmissionError::NonPositiveCandidateCost => {
            BoltV3BasketAdmissionOutcome::RejectedNonPositiveCandidateCost
        }
        BoltV3BasketAdmissionError::NonPositiveEdge => {
            BoltV3BasketAdmissionOutcome::RejectedNonPositiveEdge
        }
        BoltV3BasketAdmissionError::EdgeThreshold => {
            BoltV3BasketAdmissionOutcome::RejectedEdgeThreshold
        }
        BoltV3BasketAdmissionError::MissingGroupingProof => {
            BoltV3BasketAdmissionOutcome::RejectedMissingGroupingProof
        }
        BoltV3BasketAdmissionError::MissingSettlementRules => {
            BoltV3BasketAdmissionOutcome::RejectedMissingSettlementRules
        }
        BoltV3BasketAdmissionError::RetryBudgetExceeded => {
            BoltV3BasketAdmissionOutcome::RejectedRetryBudgetExceeded
        }
        BoltV3BasketAdmissionError::BasketAlreadyOpen
        | BoltV3BasketAdmissionError::BasketReservationMissing
        | BoltV3BasketAdmissionError::SubmitAdmissionFailed(_)
        | BoltV3BasketAdmissionError::EvidenceWriteFailed { .. } => {
            BoltV3BasketAdmissionOutcome::RejectedSubmitSlots
        }
    }
}
