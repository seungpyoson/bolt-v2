use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use crate::bolt_v3_current_evidence::{
    BasketAdmissionDetails, BasketAdmissionRejectedFact, BasketAdmissionRejectionReason,
    DecisionEvidenceRecorder, NonBlockingRecordOutcome,
};
use crate::bolt_v3_outcome_group_scanner::OutcomeGroupScanEvidence;
use crate::bolt_v3_outcome_group_sources::outcome_group_observation_is_fresh;
use crate::bolt_v3_outcome_groups::{OutcomeGroup, ValidatedOutcomeGroup};
use crate::bolt_v3_submit_admission::{
    BoltV3BasketSubmitSlotClaim, BoltV3SubmitAdmissionError, BoltV3SubmitAdmissionPermit,
    BoltV3SubmitAdmissionState,
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
    SubmitClaimsMismatch,
    MissingGroupingProof,
    GroupingProofMismatch,
    MissingSettlementRules,
    RetryBudgetExceeded,
    BasketAlreadyOpen,
    BasketReservationMissing,
    StuckReservationHeld,
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
            Self::SubmitClaimsMismatch => {
                write!(f, "basket admission submit claims must match scanned legs")
            }
            Self::MissingGroupingProof => write!(f, "basket admission missing grouping proof"),
            Self::GroupingProofMismatch => {
                write!(
                    f,
                    "basket admission scanner grouping proof does not match group"
                )
            }
            Self::MissingSettlementRules => write!(f, "basket admission missing settlement rules"),
            Self::RetryBudgetExceeded => write!(f, "basket admission retry budget exceeded"),
            Self::BasketAlreadyOpen => write!(f, "basket admission basket id is already open"),
            Self::BasketReservationMissing => {
                write!(f, "basket admission reservation does not exist")
            }
            Self::StuckReservationHeld => {
                write!(f, "basket admission stuck reservation must remain held")
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
    inner: Arc<Mutex<BoltV3BasketAdmissionInner>>,
    decision_evidence: Arc<DecisionEvidenceRecorder>,
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
    release_on_drop: bool,
}

#[derive(Debug)]
pub struct BoltV3BasketAdmissionPermit {
    inner: Arc<Mutex<BoltV3BasketAdmissionInner>>,
    basket_id: String,
    submit_permit: Option<BoltV3SubmitAdmissionPermit>,
}

impl BoltV3BasketAdmissionPermit {
    pub fn commit_submitted(&mut self) {
        if let Some(submit_permit) = self.submit_permit.take() {
            submit_permit.commit_submitted();
        }
    }
}

impl Drop for BoltV3BasketAdmissionPermit {
    fn drop(&mut self) {
        let mut inner = self
            .inner
            .lock()
            .expect("basket admission state mutex should not be poisoned");
        let should_release = inner
            .open_baskets
            .get(self.basket_id.as_str())
            .is_some_and(|reservation| reservation.release_on_drop);
        if should_release {
            inner.open_baskets.remove(self.basket_id.as_str());
        }
    }
}

impl BoltV3BasketAdmissionState {
    pub fn new(
        decision_evidence: Arc<DecisionEvidenceRecorder>,
        limits: BoltV3BasketAdmissionLimits,
    ) -> Self {
        Self {
            limits,
            inner: Arc::new(Mutex::new(BoltV3BasketAdmissionInner {
                open_baskets: BTreeMap::new(),
            })),
            decision_evidence,
        }
    }

    pub fn admit(
        &self,
        request: &BoltV3BasketAdmissionRequest<'_>,
        submit_admission: &BoltV3SubmitAdmissionState,
    ) -> Result<BoltV3BasketAdmissionPermit, BoltV3BasketAdmissionError> {
        if let Err(error) = self.evaluate_basket_request(request) {
            self.record_basket_rejection(request, basket_rejection_reason(&error))?;
            return Err(error);
        }
        let details = basket_admission_details(request)?;
        if let Err(error) = self.reserve_open_basket(request) {
            self.record_basket_rejection(request, basket_rejection_reason(&error))?;
            return Err(error);
        }

        let submit_permit = match submit_admission.reserve_basket_submit_slots_with_evidence(
            request.execution_client_id,
            &request.submit_claims,
            &details,
            |fact| self.decision_evidence.record_basket_admission_granted(fact),
        ) {
            Ok(permit) => permit,
            Err(error) => {
                self.release_reserved_basket_after_failed_submit(request.basket_id);
                self.record_basket_rejection(request, BasketAdmissionRejectionReason::SubmitSlots)?;
                return Err(BoltV3BasketAdmissionError::SubmitAdmissionFailed(error));
            }
        };
        Ok(BoltV3BasketAdmissionPermit {
            inner: Arc::clone(&self.inner),
            basket_id: request.basket_id.to_string(),
            submit_permit: Some(submit_permit),
        })
    }

    pub fn release_basket(
        &self,
        basket_id: &str,
        reason: BoltV3BasketAdmissionReleaseReason,
    ) -> Result<(), BoltV3BasketAdmissionError> {
        let mut inner = self
            .inner
            .lock()
            .expect("basket admission state mutex should not be poisoned");
        if reason == BoltV3BasketAdmissionReleaseReason::Stuck
            && inner.open_baskets.contains_key(basket_id)
        {
            if let Some(reservation) = inner.open_baskets.get_mut(basket_id) {
                reservation.release_on_drop = false;
            }
            return Err(BoltV3BasketAdmissionError::StuckReservationHeld);
        }
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
        let Some(grouping_proof) = request.group.grouping_proof.as_ref() else {
            return Err(BoltV3BasketAdmissionError::MissingGroupingProof);
        };
        let Some(scanner_grouping_proof) = request.scanner_evidence.grouping_proof.as_ref() else {
            return Err(BoltV3BasketAdmissionError::MissingGroupingProof);
        };
        if request.scanner_evidence.group_id != request.group.group_id
            || scanner_grouping_proof != grouping_proof
        {
            return Err(BoltV3BasketAdmissionError::GroupingProofMismatch);
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
        self.validate_submit_claims_match_scanned_legs(request)?;
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
            !outcome_group_observation_is_fresh(
                request.now_unix_ms,
                leg.observed_unix_ms,
                self.limits.max_scanner_evidence_age_ms,
                None,
            )
        }) {
            return Err(BoltV3BasketAdmissionError::StaleScannerEvidence);
        }
        if !outcome_group_observation_is_fresh(
            request.now_unix_ms,
            request.submit_recheck_observed_unix_ms,
            self.limits.max_submit_recheck_age_ms,
            None,
        ) {
            return Err(BoltV3BasketAdmissionError::StaleSubmitRecheck);
        }

        Ok(())
    }

    fn validate_submit_claims_match_scanned_legs(
        &self,
        request: &BoltV3BasketAdmissionRequest<'_>,
    ) -> Result<(), BoltV3BasketAdmissionError> {
        if request.submit_claims.len() != request.scanner_evidence.leg_costs.len() {
            return Err(BoltV3BasketAdmissionError::SubmitClaimsMismatch);
        }

        let mut scanned_notional_by_instrument = BTreeMap::new();
        for leg in &request.scanner_evidence.leg_costs {
            if !request
                .group
                .tradable_legs
                .values()
                .any(|group_leg| group_leg.instrument_id == leg.instrument_id)
            {
                return Err(BoltV3BasketAdmissionError::SubmitClaimsMismatch);
            }
            if scanned_notional_by_instrument
                .insert(leg.instrument_id.to_string(), leg.total_adjusted_cost)
                .is_some()
            {
                return Err(BoltV3BasketAdmissionError::SubmitClaimsMismatch);
            }
        }

        let mut total_claim_notional = Decimal::ZERO;
        for claim in &request.submit_claims {
            let Some(scanned_notional) =
                scanned_notional_by_instrument.remove(claim.instrument_id.as_str())
            else {
                return Err(BoltV3BasketAdmissionError::SubmitClaimsMismatch);
            };
            if claim.notional <= Decimal::ZERO || claim.notional > scanned_notional {
                return Err(BoltV3BasketAdmissionError::SubmitClaimsMismatch);
            }
            total_claim_notional = total_claim_notional
                .checked_add(claim.notional)
                .ok_or(BoltV3BasketAdmissionError::BasketNotionalCapExceeded)?;
        }

        if !scanned_notional_by_instrument.is_empty() {
            return Err(BoltV3BasketAdmissionError::SubmitClaimsMismatch);
        }
        if total_claim_notional > self.limits.max_basket_notional {
            return Err(BoltV3BasketAdmissionError::BasketNotionalCapExceeded);
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
                release_on_drop: true,
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

    fn record_basket_rejection(
        &self,
        request: &BoltV3BasketAdmissionRequest<'_>,
        reason: BasketAdmissionRejectionReason,
    ) -> Result<(), BoltV3BasketAdmissionError> {
        let details = basket_admission_details(request)?;
        if let NonBlockingRecordOutcome::Failed(error) = self
            .decision_evidence
            .record_basket_admission_rejected(BasketAdmissionRejectedFact { details, reason })
        {
            log::error!("basket admission rejection evidence failed: {error}");
        }
        Ok(())
    }
}

fn basket_admission_details(
    request: &BoltV3BasketAdmissionRequest<'_>,
) -> Result<BasketAdmissionDetails, BoltV3BasketAdmissionError> {
    let leg_order_count = u32::try_from(request.submit_claims.len()).map_err(|_| {
        BoltV3BasketAdmissionError::SubmitAdmissionFailed(
            BoltV3SubmitAdmissionError::CountCapExhausted,
        )
    })?;
    Ok(BasketAdmissionDetails {
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
    })
}

fn basket_rejection_reason(error: &BoltV3BasketAdmissionError) -> BasketAdmissionRejectionReason {
    match error {
        BoltV3BasketAdmissionError::BasketNotionalCapExceeded => {
            BasketAdmissionRejectionReason::BasketNotionalCapExceeded
        }
        BoltV3BasketAdmissionError::MaxOpenBasketCapExceeded => {
            BasketAdmissionRejectionReason::MaxOpenBasketCapExceeded
        }
        BoltV3BasketAdmissionError::StaleScannerEvidence => {
            BasketAdmissionRejectionReason::StaleScannerEvidence
        }
        BoltV3BasketAdmissionError::StaleSubmitRecheck => {
            BasketAdmissionRejectionReason::StaleSubmitRecheck
        }
        BoltV3BasketAdmissionError::NonPositiveCandidateCost => {
            BasketAdmissionRejectionReason::NonPositiveCandidateCost
        }
        BoltV3BasketAdmissionError::NonPositiveEdge => {
            BasketAdmissionRejectionReason::NonPositiveEdge
        }
        BoltV3BasketAdmissionError::EdgeThreshold => BasketAdmissionRejectionReason::EdgeThreshold,
        BoltV3BasketAdmissionError::SubmitClaimsMismatch => {
            BasketAdmissionRejectionReason::SubmitSlots
        }
        BoltV3BasketAdmissionError::MissingGroupingProof => {
            BasketAdmissionRejectionReason::MissingGroupingProof
        }
        BoltV3BasketAdmissionError::GroupingProofMismatch => {
            BasketAdmissionRejectionReason::SubmitSlots
        }
        BoltV3BasketAdmissionError::MissingSettlementRules => {
            BasketAdmissionRejectionReason::MissingSettlementRules
        }
        BoltV3BasketAdmissionError::RetryBudgetExceeded => {
            BasketAdmissionRejectionReason::RetryBudgetExceeded
        }
        BoltV3BasketAdmissionError::BasketAlreadyOpen
        | BoltV3BasketAdmissionError::BasketReservationMissing
        | BoltV3BasketAdmissionError::StuckReservationHeld
        | BoltV3BasketAdmissionError::SubmitAdmissionFailed(_)
        | BoltV3BasketAdmissionError::EvidenceWriteFailed { .. } => {
            BasketAdmissionRejectionReason::SubmitSlots
        }
    }
}
