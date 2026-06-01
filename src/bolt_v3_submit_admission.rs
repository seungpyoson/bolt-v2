use crate::bolt_v3_capital_reservation::{
    CapitalPoolSnapshot, ReservationRejectionReason, ReservationRequest,
};
use crate::bolt_v3_decision_evidence::{
    BoltV3AdmissionDecisionEvidence, BoltV3AdmissionOutcome, BoltV3DecisionEvidenceWriter,
};
use crate::bolt_v3_live_canary_gate::BoltV3LiveCanaryGateReport;
use crate::bolt_v3_loss_governor::{
    LossGovernorPolicy, LossHaltReason, LossSnapshot, evaluate_loss_admission,
};
use crate::bolt_v3_position_sizer::{
    IntentLiquidity, IntentOrderKind, IntentSide, PositionSizingAdmissionGate,
    PositionSizingGateInputs, PositionSizingLifecycleKind, PositionSizingLifecycleUpdate,
    PositionSizingRequest, ProductKind, ProductSizingSnapshot, SizingPolicy,
};
use crate::bolt_v3_sizing_state::NtDerivedSizingState;
use nautilus_model::{
    enums::{OrderSide, PositionSide},
    instruments::{Instrument, InstrumentAny},
    orders::{Order, OrderAny},
    types::Price,
};
use rust_decimal::Decimal;
use std::{
    collections::BTreeMap,
    str::FromStr,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

use crate::bolt_v3_canary_proof_policy::CANARY_PROOF_CLAIM;
pub use crate::bolt_v3_decision_evidence::BoltV3SubmitIntentKind;

const SUBMIT_ADMISSION_BPS_DENOMINATOR: u32 = 10_000;

#[derive(Debug)]
pub struct BoltV3SubmitAdmissionState {
    inner: Arc<Mutex<BoltV3SubmitAdmissionInner>>,
    decision_evidence: Arc<dyn BoltV3DecisionEvidenceWriter>,
}

#[derive(Debug)]
struct BoltV3SubmitAdmissionInner {
    gate_report: Option<BoltV3LiveCanaryGateReport>,
    admitted_order_count: u32,
    admitted_entry_order_count: u32,
    admitted_risk_reducing_exit_order_count: u32,
    admitted_replace_submit_order_count: u32,
    loss_policy: Option<LossGovernorPolicy>,
    loss_snapshot: Option<LossSnapshot>,
    position_sizer: Option<BoltV3SubmitPositionSizerState>,
}

#[derive(Debug)]
struct BoltV3SubmitPositionSizerState {
    venue_id: String,
    account_id: String,
    product_kind: ProductKind,
    collateral_currency: String,
    capital_pool: CapitalPoolSnapshot,
    policy: SizingPolicy,
    state: Option<NtDerivedSizingState>,
    gate: PositionSizingAdmissionGate,
    next_sequence: u64,
    client_order_reservations: BTreeMap<String, BoltV3SubmitReservationIndex>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BoltV3SubmitReservationIndex {
    submit_reservation_id: String,
    collateral_group_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoltV3SubmitPositionSizerConfig {
    pub venue_id: String,
    pub account_id: String,
    pub product_kind: ProductKind,
    pub collateral_currency: String,
    pub capital_pool: CapitalPoolSnapshot,
    pub policy: SizingPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoltV3SubmitPositionSizingOpenOrderReservation {
    pub client_order_id: String,
    pub submit_reservation_id: String,
    pub collateral_group_id: String,
    pub liability: Decimal,
    pub observed_at_ns: u64,
    pub evidence_label: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoltV3SubmitPositionSizingRebuildDecision {
    pub accepted: bool,
    pub reason: Option<ReservationRejectionReason>,
    pub attempted_reservation_count: usize,
    pub rebuilt_reservation_count: usize,
    pub live_reserved_liability: Decimal,
}

impl BoltV3SubmitAdmissionState {
    pub fn new_unarmed(decision_evidence: Arc<dyn BoltV3DecisionEvidenceWriter>) -> Self {
        Self::new_unarmed_with_optional_loss_governor(decision_evidence, None)
    }

    pub fn new_unarmed_with_loss_governor(
        decision_evidence: Arc<dyn BoltV3DecisionEvidenceWriter>,
        loss_policy: LossGovernorPolicy,
    ) -> Self {
        Self::new_unarmed_with_optional_loss_governor(decision_evidence, Some(loss_policy))
    }

    pub fn new_unarmed_with_position_sizer(
        decision_evidence: Arc<dyn BoltV3DecisionEvidenceWriter>,
        position_sizer: BoltV3SubmitPositionSizerConfig,
    ) -> Self {
        Self::new_unarmed_with_optional_controls(decision_evidence, None, Some(position_sizer))
    }

    pub fn new_unarmed_with_loss_governor_and_position_sizer(
        decision_evidence: Arc<dyn BoltV3DecisionEvidenceWriter>,
        loss_policy: LossGovernorPolicy,
        position_sizer: BoltV3SubmitPositionSizerConfig,
    ) -> Self {
        Self::new_unarmed_with_optional_controls(
            decision_evidence,
            Some(loss_policy),
            Some(position_sizer),
        )
    }

    fn new_unarmed_with_optional_loss_governor(
        decision_evidence: Arc<dyn BoltV3DecisionEvidenceWriter>,
        loss_policy: Option<LossGovernorPolicy>,
    ) -> Self {
        Self::new_unarmed_with_optional_controls(decision_evidence, loss_policy, None)
    }

    fn new_unarmed_with_optional_controls(
        decision_evidence: Arc<dyn BoltV3DecisionEvidenceWriter>,
        loss_policy: Option<LossGovernorPolicy>,
        position_sizer: Option<BoltV3SubmitPositionSizerConfig>,
    ) -> Self {
        Self {
            inner: Arc::new(Mutex::new(BoltV3SubmitAdmissionInner {
                gate_report: None,
                admitted_order_count: 0,
                admitted_entry_order_count: 0,
                admitted_risk_reducing_exit_order_count: 0,
                admitted_replace_submit_order_count: 0,
                loss_policy,
                loss_snapshot: None,
                position_sizer: position_sizer.map(|config| BoltV3SubmitPositionSizerState {
                    venue_id: config.venue_id,
                    account_id: config.account_id,
                    product_kind: config.product_kind,
                    collateral_currency: config.collateral_currency,
                    capital_pool: config.capital_pool,
                    policy: config.policy,
                    state: None,
                    gate: PositionSizingAdmissionGate::unreconciled(),
                    next_sequence: 0,
                    client_order_reservations: BTreeMap::new(),
                }),
            })),
            decision_evidence,
        }
    }

    pub fn arm(
        &self,
        report: BoltV3LiveCanaryGateReport,
    ) -> Result<(), BoltV3SubmitAdmissionError> {
        let mut inner = lock_inner(&self.inner);
        if inner.gate_report.is_some() {
            return Err(BoltV3SubmitAdmissionError::AlreadyArmed);
        }
        inner.gate_report = Some(report);
        inner.admitted_order_count = 0;
        inner.admitted_entry_order_count = 0;
        inner.admitted_risk_reducing_exit_order_count = 0;
        inner.admitted_replace_submit_order_count = 0;
        Ok(())
    }

    pub fn update_loss_snapshot(&self, snapshot: LossSnapshot) {
        lock_inner(&self.inner).loss_snapshot = Some(snapshot);
    }

    pub fn update_position_sizing_state(&self, state: NtDerivedSizingState) {
        if let Some(position_sizer) = lock_inner(&self.inner).position_sizer.as_mut() {
            position_sizer.capital_pool.source = state.reservation_snapshot.source.clone();
            position_sizer.capital_pool.observed_at_ns = state.reservation_snapshot.observed_at_ns;
            position_sizer.state = Some(state);
        }
    }

    pub fn position_sizer_configured(&self) -> bool {
        lock_inner(&self.inner).position_sizer.is_some()
    }

    pub fn position_sizer_live_reserved_liability(&self) -> Option<Decimal> {
        let inner = lock_inner(&self.inner);
        let position_sizer = inner.position_sizer.as_ref()?;
        Some(
            position_sizer
                .gate
                .live_reserved_liability(&position_sizer.capital_pool.pool_id),
        )
    }

    pub fn position_sizer_reconciled(&self) -> Option<bool> {
        let inner = lock_inner(&self.inner);
        let position_sizer = inner.position_sizer.as_ref()?;
        Some(position_sizer.gate.is_reconciled())
    }

    pub fn rebuild_position_sizing_open_order_reservations(
        &self,
        open_order_reservations: Vec<BoltV3SubmitPositionSizingOpenOrderReservation>,
        now_ns: u64,
    ) -> BoltV3SubmitPositionSizingRebuildDecision {
        let mut inner = lock_inner(&self.inner);
        let Some(position_sizer) = inner.position_sizer.as_mut() else {
            return BoltV3SubmitPositionSizingRebuildDecision {
                accepted: true,
                reason: None,
                attempted_reservation_count: 0,
                rebuilt_reservation_count: 0,
                live_reserved_liability: Decimal::ZERO,
            };
        };
        let Some(state) = position_sizer.state.as_ref() else {
            position_sizer.gate = PositionSizingAdmissionGate::unreconciled();
            position_sizer.client_order_reservations.clear();
            return BoltV3SubmitPositionSizingRebuildDecision {
                accepted: false,
                reason: Some(ReservationRejectionReason::MissingEvidence),
                attempted_reservation_count: 0,
                rebuilt_reservation_count: 0,
                live_reserved_liability: position_sizer
                    .gate
                    .live_reserved_liability(&position_sizer.capital_pool.pool_id),
            };
        };
        position_sizer.capital_pool.source = state.reservation_snapshot.source.clone();
        position_sizer.capital_pool.observed_at_ns = state.reservation_snapshot.observed_at_ns;

        let mut rebuilt_index = BTreeMap::new();
        let mut reservation_requests = Vec::with_capacity(open_order_reservations.len());
        for (index, reservation) in open_order_reservations.into_iter().enumerate() {
            if rebuilt_index.contains_key(&reservation.client_order_id) {
                position_sizer.gate = PositionSizingAdmissionGate::unreconciled();
                position_sizer.client_order_reservations.clear();
                return BoltV3SubmitPositionSizingRebuildDecision {
                    accepted: false,
                    reason: Some(ReservationRejectionReason::DuplicateReservation),
                    attempted_reservation_count: index + 1,
                    rebuilt_reservation_count: 0,
                    live_reserved_liability: position_sizer
                        .gate
                        .live_reserved_liability(&position_sizer.capital_pool.pool_id),
                };
            }

            let submit_reservation_id = reservation.submit_reservation_id;
            let collateral_group_id = reservation.collateral_group_id;
            rebuilt_index.insert(
                reservation.client_order_id,
                BoltV3SubmitReservationIndex {
                    submit_reservation_id: submit_reservation_id.clone(),
                    collateral_group_id: collateral_group_id.clone(),
                },
            );
            reservation_requests.push(ReservationRequest {
                request_id: submit_reservation_id,
                pool_id: position_sizer.capital_pool.pool_id.clone(),
                collateral_group_id,
                liability: reservation.liability,
                observed_at_ns: reservation.observed_at_ns,
                evidence_label: reservation.evidence_label,
            });
        }

        position_sizer.client_order_reservations.clear();
        let decision = position_sizer.gate.rebuild_open_order_reservations(
            &position_sizer.capital_pool,
            &reservation_requests,
            now_ns,
            position_sizer.policy.min_remaining_pool_balance,
        );
        if decision.accepted {
            position_sizer.client_order_reservations = rebuilt_index;
        }

        BoltV3SubmitPositionSizingRebuildDecision {
            accepted: decision.accepted,
            reason: decision.reason,
            attempted_reservation_count: decision.attempted_reservation_count,
            rebuilt_reservation_count: decision.rebuilt_reservation_count,
            live_reserved_liability: decision.live_reserved_liability,
        }
    }

    pub fn apply_position_sizing_lifecycle_update(
        &self,
        update: BoltV3SubmitPositionSizingLifecycleUpdate,
        now_ns: u64,
    ) -> BoltV3SubmitPositionSizingLifecycleDecision {
        let mut inner = lock_inner(&self.inner);
        let Some(position_sizer) = inner.position_sizer.as_mut() else {
            return BoltV3SubmitPositionSizingLifecycleDecision::unknown();
        };
        let Some(index) = position_sizer
            .client_order_reservations
            .get(&update.client_order_id)
            .cloned()
        else {
            log::warn!(
                "bolt-v3 submit admission received position-sizer lifecycle update for unknown client_order_id={}",
                update.client_order_id
            );
            return BoltV3SubmitPositionSizingLifecycleDecision::unknown();
        };
        let lifecycle_update = PositionSizingLifecycleUpdate {
            intent_id: index.submit_reservation_id.clone(),
            pool_id: position_sizer.capital_pool.pool_id.clone(),
            collateral_group_id: update.collateral_group_id,
            remaining_liability: update.remaining_liability,
            observed_at_ns: update.observed_at_ns,
            evidence_label: update.evidence_label,
            kind: update.kind,
        };
        let decision = position_sizer.gate.apply_lifecycle_update(
            &position_sizer.capital_pool,
            &lifecycle_update,
            now_ns,
            position_sizer.policy.min_remaining_pool_balance,
        );
        if decision.accepted
            && update.kind == PositionSizingLifecycleKind::Terminal
            && position_sizer
                .client_order_reservations
                .get(&update.client_order_id)
                .map(|current| current.submit_reservation_id.as_str())
                == Some(index.submit_reservation_id.as_str())
        {
            position_sizer
                .client_order_reservations
                .remove(&update.client_order_id);
        }
        BoltV3SubmitPositionSizingLifecycleDecision {
            accepted: decision.accepted,
            unknown_reservation: false,
        }
    }

    pub fn apply_position_sizing_terminal_order_event(
        &self,
        client_order_id: String,
        observed_at_ns: u64,
        evidence_label: String,
    ) -> BoltV3SubmitPositionSizingLifecycleDecision {
        let mut inner = lock_inner(&self.inner);
        let Some(position_sizer) = inner.position_sizer.as_mut() else {
            return BoltV3SubmitPositionSizingLifecycleDecision::unknown();
        };
        let Some(index) = position_sizer
            .client_order_reservations
            .get(&client_order_id)
            .cloned()
        else {
            log::warn!(
                "bolt-v3 submit admission received position-sizer terminal order event for unknown client_order_id={}",
                client_order_id
            );
            return BoltV3SubmitPositionSizingLifecycleDecision::unknown();
        };
        let lifecycle_update = PositionSizingLifecycleUpdate {
            intent_id: index.submit_reservation_id.clone(),
            pool_id: position_sizer.capital_pool.pool_id.clone(),
            collateral_group_id: index.collateral_group_id,
            remaining_liability: Decimal::ZERO,
            observed_at_ns,
            evidence_label,
            kind: PositionSizingLifecycleKind::Terminal,
        };
        let decision = position_sizer.gate.apply_lifecycle_update(
            &position_sizer.capital_pool,
            &lifecycle_update,
            observed_at_ns,
            position_sizer.policy.min_remaining_pool_balance,
        );
        if decision.accepted
            && position_sizer
                .client_order_reservations
                .get(&client_order_id)
                .map(|current| current.submit_reservation_id.as_str())
                == Some(index.submit_reservation_id.as_str())
        {
            position_sizer
                .client_order_reservations
                .remove(&client_order_id);
        }
        BoltV3SubmitPositionSizingLifecycleDecision {
            accepted: decision.accepted,
            unknown_reservation: false,
        }
    }

    pub fn admit(
        &self,
        request: &BoltV3SubmitAdmissionRequest,
    ) -> Result<BoltV3SubmitAdmissionPermit, BoltV3SubmitAdmissionError> {
        self.admit_with_clock(request, current_unix_ns()?)
    }

    pub fn admit_at(
        &self,
        request: &BoltV3SubmitAdmissionRequest,
        now_ns: u64,
    ) -> Result<BoltV3SubmitAdmissionPermit, BoltV3SubmitAdmissionError> {
        self.admit_with_clock(request, now_ns)
    }

    fn admit_with_clock(
        &self,
        request: &BoltV3SubmitAdmissionRequest,
        now_ns: u64,
    ) -> Result<BoltV3SubmitAdmissionPermit, BoltV3SubmitAdmissionError> {
        let mut inner = lock_inner(&self.inner);
        let evaluation = Self::evaluate(&mut inner, request, now_ns);
        let evidence = BoltV3AdmissionDecisionEvidence {
            strategy_id: request.strategy_id.clone(),
            client_order_id: request.client_order_id.clone(),
            instrument_id: request.instrument_id.clone(),
            notional: request.notional.to_string(),
            intent_kind: request.intent_kind,
            outcome: evaluation.outcome.clone(),
            loss_halt_reasons: evaluation
                .loss_halt_reasons
                .iter()
                .map(|reason| reason.as_str().to_string())
                .collect(),
        };
        if let Err(err) = self.decision_evidence.record_admission_decision(&evidence) {
            if let Some(rollback) = evaluation.rollback.as_ref() {
                rollback_position_sizer_reservation(&mut inner, rollback);
            }
            return Err(BoltV3SubmitAdmissionError::EvidenceWriteFailed {
                reason: format!("{err:#}"),
            });
        }
        match evaluation.outcome {
            BoltV3AdmissionOutcome::Admitted => {
                inner.admitted_order_count += 1;
                match request.intent_kind {
                    BoltV3SubmitIntentKind::Entry => {
                        inner.admitted_entry_order_count += 1;
                    }
                    BoltV3SubmitIntentKind::RiskReducingExit => {
                        inner.admitted_risk_reducing_exit_order_count += 1;
                    }
                    BoltV3SubmitIntentKind::ReplaceSubmit => {
                        inner.admitted_replace_submit_order_count += 1;
                    }
                }
                Ok(BoltV3SubmitAdmissionPermit {
                    inner: self.inner.clone(),
                    rollback: evaluation.rollback,
                    committed: false,
                })
            }
            BoltV3AdmissionOutcome::RejectedNotArmed => Err(BoltV3SubmitAdmissionError::NotArmed),
            BoltV3AdmissionOutcome::RejectedSubmitLifecycleDisallowed => {
                Err(BoltV3SubmitAdmissionError::SubmitLifecycleDisallowed {
                    intent: request.intent_kind,
                })
            }
            BoltV3AdmissionOutcome::RejectedLossGovernorHalted => {
                Err(BoltV3SubmitAdmissionError::LossGovernorHalted {
                    reasons: evaluation.loss_halt_reasons,
                })
            }
            BoltV3AdmissionOutcome::RejectedNonPositiveNotional => {
                Err(BoltV3SubmitAdmissionError::NonPositiveNotional)
            }
            BoltV3AdmissionOutcome::RejectedNotionalCapExceeded => {
                Err(BoltV3SubmitAdmissionError::NotionalCapExceeded)
            }
            BoltV3AdmissionOutcome::RejectedInvalidCanaryProofClaim => {
                Err(BoltV3SubmitAdmissionError::InvalidCanaryProofClaim)
            }
            BoltV3AdmissionOutcome::RejectedInvalidRiskReducingExitProof => {
                Err(BoltV3SubmitAdmissionError::InvalidRiskReducingExitProof)
            }
            BoltV3AdmissionOutcome::RejectedCountCapExhausted => {
                Err(BoltV3SubmitAdmissionError::CountCapExhausted)
            }
            BoltV3AdmissionOutcome::RejectedPositionSizing => {
                Err(BoltV3SubmitAdmissionError::PositionSizingRejected {
                    reason: evaluation
                        .position_sizer_rejection
                        .unwrap_or(BoltV3PositionSizerRejectReason::Rejected),
                })
            }
        }
    }

    fn evaluate(
        inner: &mut BoltV3SubmitAdmissionInner,
        request: &BoltV3SubmitAdmissionRequest,
        now_ns: u64,
    ) -> BoltV3SubmitAdmissionEvaluation {
        let Some(report) = inner.gate_report.as_ref() else {
            return BoltV3SubmitAdmissionEvaluation::without_loss_halt(
                BoltV3AdmissionOutcome::RejectedNotArmed,
            );
        };
        if !request.lifecycle_policy.allows(request.intent_kind) {
            return BoltV3SubmitAdmissionEvaluation::without_loss_halt(
                BoltV3AdmissionOutcome::RejectedSubmitLifecycleDisallowed,
            );
        }
        if let Some(loss_policy) = inner.loss_policy.as_ref()
            && matches!(
                request.intent_kind,
                BoltV3SubmitIntentKind::Entry | BoltV3SubmitIntentKind::ReplaceSubmit
            )
        {
            let decision =
                evaluate_loss_admission(loss_policy, inner.loss_snapshot.as_ref(), now_ns);
            if !decision.accepted {
                return BoltV3SubmitAdmissionEvaluation::loss_halt(decision.halt_reasons);
            }
        }
        if request.notional <= Decimal::ZERO {
            return BoltV3SubmitAdmissionEvaluation::without_loss_halt(
                BoltV3AdmissionOutcome::RejectedNonPositiveNotional,
            );
        }
        if matches!(
            request.intent_kind,
            BoltV3SubmitIntentKind::Entry | BoltV3SubmitIntentKind::ReplaceSubmit
        ) && request.notional > report.max_notional_per_order()
        {
            return BoltV3SubmitAdmissionEvaluation::without_loss_halt(
                BoltV3AdmissionOutcome::RejectedNotionalCapExceeded,
            );
        }
        if request
            .canary_proof_claim
            .as_deref()
            .is_some_and(|claim| claim != CANARY_PROOF_CLAIM)
        {
            return BoltV3SubmitAdmissionEvaluation::without_loss_halt(
                BoltV3AdmissionOutcome::RejectedInvalidCanaryProofClaim,
            );
        }
        match request.intent_kind {
            BoltV3SubmitIntentKind::Entry => {
                if inner.admitted_entry_order_count >= report.max_live_entry_order_count() {
                    return BoltV3SubmitAdmissionEvaluation::without_loss_halt(
                        BoltV3AdmissionOutcome::RejectedCountCapExhausted,
                    );
                }
            }
            BoltV3SubmitIntentKind::RiskReducingExit => {
                let Some(proof) = request.risk_reducing_exit_proof.as_ref() else {
                    return BoltV3SubmitAdmissionEvaluation::without_loss_halt(
                        BoltV3AdmissionOutcome::RejectedInvalidRiskReducingExitProof,
                    );
                };
                if !proof.is_valid_for(request) {
                    return BoltV3SubmitAdmissionEvaluation::without_loss_halt(
                        BoltV3AdmissionOutcome::RejectedInvalidRiskReducingExitProof,
                    );
                }
                if inner.admitted_risk_reducing_exit_order_count
                    >= report.max_live_risk_reducing_exit_order_count()
                {
                    return BoltV3SubmitAdmissionEvaluation::without_loss_halt(
                        BoltV3AdmissionOutcome::RejectedCountCapExhausted,
                    );
                }
            }
            BoltV3SubmitIntentKind::ReplaceSubmit => {
                if inner.admitted_replace_submit_order_count
                    >= report.max_live_replace_submit_order_count()
                {
                    return BoltV3SubmitAdmissionEvaluation::without_loss_halt(
                        BoltV3AdmissionOutcome::RejectedCountCapExhausted,
                    );
                }
            }
        }
        if inner.position_sizer.is_some() {
            let decision = evaluate_position_sizer_submit(inner, request, now_ns);
            if !decision.accepted {
                return BoltV3SubmitAdmissionEvaluation::position_sizer_rejected(decision.reason);
            }
            return BoltV3SubmitAdmissionEvaluation::admitted_with_rollback(decision.rollback);
        }
        BoltV3SubmitAdmissionEvaluation::without_loss_halt(BoltV3AdmissionOutcome::Admitted)
    }

    pub fn admitted_order_count(&self) -> u32 {
        lock_inner(&self.inner).admitted_order_count
    }

    /// Gate-approved maximum reference-quote age (seconds) carried by the armed
    /// gate report, or `None` when the state is not yet armed. This is the single
    /// authoritative freshness bound for the armed live path (A5): the submit /
    /// forced-flat stale check plumbs this value in so the gate-validated
    /// freshness policy — not an independent strategy-config value — governs
    /// whether a reference quote is fresh enough to keep trading. `None` (unarmed)
    /// is irrelevant to live money because admission rejects every order until the
    /// state is armed.
    pub fn reference_quote_max_age_seconds(&self) -> Option<u64> {
        self.inner
            .lock()
            .expect("submit admission state mutex should not be poisoned")
            .gate_report
            .as_ref()
            .map(BoltV3LiveCanaryGateReport::reference_quote_max_age_seconds)
    }

    pub fn loss_governor_configured(&self) -> bool {
        lock_inner(&self.inner).loss_policy.is_some()
    }
}

#[derive(Debug)]
pub struct BoltV3SubmitAdmissionPermit {
    inner: Arc<Mutex<BoltV3SubmitAdmissionInner>>,
    rollback: Option<BoltV3PositionSizerReservationRollback>,
    committed: bool,
}

impl BoltV3SubmitAdmissionPermit {
    pub fn commit_submitted(mut self) {
        self.committed = true;
        self.rollback = None;
    }
}

impl Drop for BoltV3SubmitAdmissionPermit {
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        let Some(rollback) = self.rollback.as_ref() else {
            return;
        };
        // Invariant: callers must not drop an uncommitted permit while already
        // holding the admission lock; rollback is deliberately fail-closed.
        let mut inner = lock_inner(&self.inner);
        rollback_position_sizer_reservation(&mut inner, rollback);
    }
}

#[derive(Debug)]
struct BoltV3SubmitAdmissionEvaluation {
    outcome: BoltV3AdmissionOutcome,
    loss_halt_reasons: Vec<LossHaltReason>,
    position_sizer_rejection: Option<BoltV3PositionSizerRejectReason>,
    rollback: Option<BoltV3PositionSizerReservationRollback>,
}

impl BoltV3SubmitAdmissionEvaluation {
    fn without_loss_halt(outcome: BoltV3AdmissionOutcome) -> Self {
        Self {
            outcome,
            loss_halt_reasons: Vec::new(),
            position_sizer_rejection: None,
            rollback: None,
        }
    }

    fn loss_halt(loss_halt_reasons: Vec<LossHaltReason>) -> Self {
        Self {
            outcome: BoltV3AdmissionOutcome::RejectedLossGovernorHalted,
            loss_halt_reasons,
            position_sizer_rejection: None,
            rollback: None,
        }
    }

    fn position_sizer_rejected(reason: BoltV3PositionSizerRejectReason) -> Self {
        Self {
            outcome: BoltV3AdmissionOutcome::RejectedPositionSizing,
            loss_halt_reasons: Vec::new(),
            position_sizer_rejection: Some(reason),
            rollback: None,
        }
    }

    fn admitted_with_rollback(rollback: Option<BoltV3PositionSizerReservationRollback>) -> Self {
        Self {
            outcome: BoltV3AdmissionOutcome::Admitted,
            loss_halt_reasons: Vec::new(),
            position_sizer_rejection: None,
            rollback,
        }
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum BoltV3OrderLifecycleIntent {
    Entry,
    RiskReducingExit,
    ReplaceSubmit,
    PlainCancel,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoltV3RiskReducingExitProof {
    pub position_id: String,
    pub instrument_id: String,
    pub position_side: PositionSide,
    pub exit_order_side: OrderSide,
    pub position_quantity: Decimal,
    pub exit_quantity: Decimal,
}

impl BoltV3RiskReducingExitProof {
    fn is_valid_for(&self, request: &BoltV3SubmitAdmissionRequest) -> bool {
        self.instrument_id == request.instrument_id
            && self.exit_order_side == request.order_side
            && self.exit_quantity == request.order_quantity
            && self.position_quantity > Decimal::ZERO
            && self.exit_quantity > Decimal::ZERO
            && self.exit_quantity <= self.position_quantity
            && matches!(
                (self.position_side, self.exit_order_side),
                (PositionSide::Long, OrderSide::Sell) | (PositionSide::Short, OrderSide::Buy)
            )
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct BoltV3SubmitLifecyclePolicy {
    replace_submit: bool,
}

impl BoltV3SubmitLifecyclePolicy {
    pub fn new(replace_submit: bool) -> Self {
        Self { replace_submit }
    }

    pub fn submit_intent_for(
        &self,
        intent: BoltV3OrderLifecycleIntent,
    ) -> Result<Option<BoltV3SubmitIntentKind>, BoltV3SubmitAdmissionError> {
        match intent {
            BoltV3OrderLifecycleIntent::Entry => Ok(Some(BoltV3SubmitIntentKind::Entry)),
            BoltV3OrderLifecycleIntent::RiskReducingExit => {
                Ok(Some(BoltV3SubmitIntentKind::RiskReducingExit))
            }
            BoltV3OrderLifecycleIntent::ReplaceSubmit if self.replace_submit => {
                Ok(Some(BoltV3SubmitIntentKind::ReplaceSubmit))
            }
            BoltV3OrderLifecycleIntent::ReplaceSubmit => Ok(None),
            BoltV3OrderLifecycleIntent::PlainCancel => Ok(None),
        }
    }

    fn allows(&self, intent: BoltV3SubmitIntentKind) -> bool {
        match intent {
            BoltV3SubmitIntentKind::Entry | BoltV3SubmitIntentKind::RiskReducingExit => true,
            BoltV3SubmitIntentKind::ReplaceSubmit => self.replace_submit,
        }
    }
}

#[derive(Debug)]
pub struct BoltV3SubmitAdmissionRequest {
    pub strategy_id: String,
    pub client_order_id: String,
    pub instrument_id: String,
    pub notional: Decimal,
    pub order_side: OrderSide,
    pub order_quantity: Decimal,
    pub intent_kind: BoltV3SubmitIntentKind,
    pub lifecycle_policy: BoltV3SubmitLifecyclePolicy,
    pub canary_proof_claim: Option<String>,
    pub risk_reducing_exit_proof: Option<BoltV3RiskReducingExitProof>,
    pub position_sizing: Option<BoltV3CompiledOrderSizingEvidence>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoltV3CompiledOrderSizingEvidence {
    pub venue_id: String,
    pub product_kind: BoltV3CompiledProductKind,
    pub side: BoltV3CompiledOrderSide,
    pub quantity: Decimal,
    pub effective_price: Decimal,
    pub order_kind: BoltV3CompiledOrderKind,
    pub liquidity: BoltV3CompiledOrderLiquidity,
    pub quote_set_id: Option<String>,
    pub prediction_market_outcome: Option<PredictionMarketOutcomeSide>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoltV3CompiledProductKind {
    PredictionMarketBinary,
}

impl BoltV3CompiledProductKind {
    fn to_position_sizer(self) -> ProductKind {
        match self {
            Self::PredictionMarketBinary => ProductKind::PredictionMarketBinary,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoltV3CompiledOrderSide {
    Buy,
    Sell,
}

impl BoltV3CompiledOrderSide {
    fn to_position_sizer(self) -> IntentSide {
        match self {
            Self::Buy => IntentSide::Buy,
            Self::Sell => IntentSide::Sell,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoltV3CompiledOrderKind {
    Limit,
}

impl BoltV3CompiledOrderKind {
    fn to_position_sizer(self) -> IntentOrderKind {
        match self {
            Self::Limit => IntentOrderKind::Limit,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoltV3CompiledOrderLiquidity {
    Taker,
    RestingMaker,
}

impl BoltV3CompiledOrderLiquidity {
    fn to_position_sizer(self) -> IntentLiquidity {
        match self {
            Self::Taker => IntentLiquidity::Taker,
            Self::RestingMaker => IntentLiquidity::RestingMaker,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PredictionMarketOutcomeSide {
    Yes,
    No,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BoltV3PositionSizerReservationRollback {
    client_order_id: String,
    submit_reservation_id: String,
    pool_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BoltV3PositionSizerSubmitDecision {
    accepted: bool,
    reason: BoltV3PositionSizerRejectReason,
    rollback: Option<BoltV3PositionSizerReservationRollback>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoltV3PositionSizerRejectReason {
    Rejected,
    MissingSizingEvidence,
    VenueMismatch,
    AccountMismatch,
    ProductKindMismatch,
    CollateralCurrencyMismatch,
    UnsupportedProductKind,
    MissingPredictionMarketOutcome,
    OutcomeInstrumentMismatch,
    ReplaceSubmitUnsupported,
    DuplicateClientOrderId,
    MissingNtState,
    StaleNtState,
    UnattributedNtState,
    ReconciliationRequired,
    OverBudget,
    SizingRejected,
    SizedQuantityMismatch,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoltV3SubmitPositionSizingLifecycleUpdate {
    pub client_order_id: String,
    pub collateral_group_id: String,
    pub remaining_liability: Decimal,
    pub observed_at_ns: u64,
    pub evidence_label: String,
    pub kind: PositionSizingLifecycleKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoltV3SubmitPositionSizingLifecycleDecision {
    pub accepted: bool,
    pub unknown_reservation: bool,
}

impl BoltV3SubmitPositionSizingLifecycleDecision {
    fn unknown() -> Self {
        Self {
            accepted: true,
            unknown_reservation: true,
        }
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct BoltV3QuoteQuantityAdmissionInput {
    pub order_side: BoltV3QuoteQuantityOrderSide,
    pub is_quote_quantity: bool,
    pub is_inverse: bool,
    pub submitted_quote_quantity: Decimal,
    pub calculated_notional: Decimal,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum BoltV3QuoteQuantityOrderSide {
    Buy,
    Sell,
    Other,
}

/// Floor a quote-quantity order's admission notional at the submitted quote
/// quantity. A non-inverse quote-quantity order commits exactly its submitted
/// quote quantity in settlement currency — on either side and for ANY order kind
/// (Limit, StopLimit, Market, …) — so the per-order cap must never be checked
/// against a smaller value.
///
/// When the venue rounds the derived base quantity DOWN to size precision, NT's
/// effective notional can land a sub-tick BELOW the committed quote quantity. The
/// floor is therefore applied to every non-inverse quote-quantity Buy/Sell order
/// regardless of kind — restricting it (to one side, or to Limit/StopLimit) would
/// leave the excluded shapes' safety dependent on a precision coincidence rather
/// than a structural guarantee. The conservative effective-price pull that feeds
/// `calculated_notional` is a separate, Limit/StopLimit-only concern handled in
/// [`quote_quantity_effective_price`]; this floor is the kind-independent
/// backstop. Inverse instruments do not denominate the quote quantity in
/// settlement currency, so the floor is skipped for them.
pub fn conservative_quote_quantity_admission_notional(
    input: BoltV3QuoteQuantityAdmissionInput,
) -> Decimal {
    if input.is_quote_quantity
        && !input.is_inverse
        && matches!(
            input.order_side,
            BoltV3QuoteQuantityOrderSide::Buy | BoltV3QuoteQuantityOrderSide::Sell
        )
    {
        return input
            .calculated_notional
            .max(input.submitted_quote_quantity);
    }
    input.calculated_notional
}

/// Admission base notional for a BASE-quantity order: the product of the
/// already-rounded order's price and quantity. This is the single definition of
/// the base-quantity notional; every submit path (and the base-only test helper)
/// derives it from here so there is no divergent `price * quantity` copy.
pub fn base_quantity_admission_notional(order_price: Decimal, order_quantity: Decimal) -> Decimal {
    order_price * order_quantity
}

/// Single source of truth for "given a built venue-precision order, its
/// instrument, and a conservative reference/last price, what is the conservative
/// admission BASE notional?"
///
/// Both submit paths derive their admission notional from a built order through
/// THIS function so base-quantity and quote-quantity orders are sized
/// identically everywhere. Divergent per-call-site notional math is forbidden
/// (NO DUAL PATHS): a price*quantity shortcut UNDERSTATES the real cash debit of
/// a quote-quantity (quote-currency-denominated) order, understating the
/// per-order cap.
///
/// Contract for the inputs:
/// - `order_price` / `order_quantity` are the Decimal price and quantity of the
///   already-rounded order actually handed to the venue. For a BASE-quantity
///   order the result is exactly `order_price * order_quantity`, unchanged from
///   the historical per-call-site computation.
/// - `last_price` is the conservative reference/last price used to value a
///   quote-quantity order. For a quote-quantity order it MUST be `Some`; when a
///   caller cannot resolve a reference price it passes `None` and this function
///   returns `None` so the caller can apply its own degraded fallback. It is
///   ignored for base-quantity orders.
/// - `quote_reference_price` is the side-appropriate top-of-book price (best ask
///   for a BUY, best bid for a SELL) used to pick a conservative effective price
///   for the quote→base conversion. `None` means no top-of-book is available, in
///   which case the effective price is the `last_price` (matching the historical
///   no-quote-tick fallback).
///
/// For a quote-quantity, non-inverse Limit/StopLimit order the effective price is
/// pulled toward the book (`min(last, ask)` for a BUY, `max(last, bid)` for a
/// SELL) so the quote→base conversion yields the LARGEST base quantity the order
/// could fill — the conservative direction that never understates the notional.
/// The result is then floored by [`conservative_quote_quantity_admission_notional`].
pub fn admission_base_notional_from_order(
    order: &OrderAny,
    instrument: &InstrumentAny,
    order_price: Decimal,
    order_quantity: Decimal,
    last_price: Option<Price>,
    quote_reference_price: Option<Price>,
) -> Option<Decimal> {
    if !order.is_quote_quantity() {
        return Some(base_quantity_admission_notional(
            order_price,
            order_quantity,
        ));
    }
    // Fail CLOSED on an inverse quote-quantity order at the SHARED admission
    // helper (A6). An inverse instrument denominates the quote quantity in the
    // QUOTE currency, not the settlement currency, so neither
    // `calculate_notional_value` here nor the submitted-quote-quantity floor in
    // [`conservative_quote_quantity_admission_notional`] yields a settlement-
    // currency notional the per-order cap can be checked against — both would
    // UNDERSTATE the real cash debit. This is the single, structural rejection
    // point: returning `None` makes every caller (production strategy, canary
    // proof executor) treat an inverse quote-quantity order as unvaluable and
    // refuse it, rather than relying on a per-caller fallback to notice the
    // inverse case. This system trades only non-inverse binary options; carrying
    // currency-aware settlement notional would be the alternative, but the
    // fail-closed reject is the conservative default until such an instrument is
    // intentionally supported. Reachable only if an inverse instrument enters the
    // universe (the market-family filters gate it out today), but the defense
    // lives here so the cap can never be silently understated.
    if instrument.is_inverse() {
        return None;
    }
    let last_px = last_price?;
    let effective_price =
        quote_quantity_effective_price(order, instrument, last_px, quote_reference_price);
    let effective_quantity = instrument.calculate_base_quantity(order.quantity(), effective_price);
    let calculated_notional = instrument
        .calculate_notional_value(effective_quantity, last_px, Some(true))
        .as_decimal();
    let submitted_quote_quantity = Decimal::from_str(order.quantity().to_string().trim()).ok()?;
    Some(conservative_quote_quantity_admission_notional(
        BoltV3QuoteQuantityAdmissionInput {
            order_side: match order.order_side() {
                OrderSide::Buy => BoltV3QuoteQuantityOrderSide::Buy,
                OrderSide::Sell => BoltV3QuoteQuantityOrderSide::Sell,
                _ => BoltV3QuoteQuantityOrderSide::Other,
            },
            is_quote_quantity: order.is_quote_quantity(),
            is_inverse: instrument.is_inverse(),
            submitted_quote_quantity,
            calculated_notional,
        },
    ))
}

/// Conservative effective price for the quote→base conversion of a
/// quote-quantity order. Mirrors the production cache-driven selection: for a
/// non-inverse Limit/StopLimit order it pulls the price toward the book
/// (`min(last, ask)` for a BUY, `max(last, bid)` for a SELL) so a smaller
/// effective price yields a larger base quantity — the conservative direction.
/// Every other shape, or a missing top-of-book, falls back to `last_price`.
fn quote_quantity_effective_price(
    order: &OrderAny,
    instrument: &InstrumentAny,
    last_price: Price,
    quote_reference_price: Option<Price>,
) -> Price {
    if !order.is_quote_quantity()
        || instrument.is_inverse()
        || !matches!(order, OrderAny::Limit(_) | OrderAny::StopLimit(_))
    {
        return last_price;
    }
    let Some(quote_reference_price) = quote_reference_price else {
        return last_price;
    };
    match order.order_side() {
        OrderSide::Buy => last_price.min(quote_reference_price),
        OrderSide::Sell => last_price.max(quote_reference_price),
        _ => last_price,
    }
}

pub fn fee_inclusive_admission_notional(notional: Decimal, max_fee_bps: Decimal) -> Decimal {
    let fee_multiplier =
        Decimal::ONE + max_fee_bps / Decimal::from(SUBMIT_ADMISSION_BPS_DENOMINATOR);
    notional * fee_multiplier
}

/// Cap-bypass-via-rounding guard for submit paths that carry an operator
/// intent SEPARATE from the order actually built.
///
/// Callers must pass the base notional of the already-rounded order
/// (`rounded_base_notional`) — i.e. the product of the venue-precision
/// `Price`/`Quantity` actually submitted — together with the operator-intended
/// raw notional that authorized the order. Banker's rounding to venue precision
/// can round a quantity or price UP, so the rounded base notional can exceed the
/// intended notional. When that happens this helper fails CLOSED: a rounded
/// order may never debit more than the operator approved, so admission is
/// refused rather than letting the cap be bypassed by rounding.
///
/// On success it returns the fee-inclusive admission notional computed from the
/// rounded base, so the cap check downstream sees the same cash debit the venue
/// will incur.
///
/// Scope: this guard is required precisely where the operator approves an
/// explicit `order_intent.notional` BEFORE the venue-precision order is
/// constructed — currently the canary proof executor. The production strategy
/// path does NOT use this guard and structurally does not need it: it builds
/// the venue-precision order first and derives its admission notional from that
/// already-rounded order (`binary_oracle_edge_taker::submit_admission_request_from_order`,
/// whose intent is `BoltV3OrderIntentEvidence::from_compiled_order`), so the
/// strict-`>` cap check in [`BoltV3SubmitAdmissionState::admit`] already
/// evaluates the exact order handed to the venue — there is no separate
/// unrounded intent for rounding to bypass. Both paths share the same
/// fee-inclusive cap arithmetic via [`fee_inclusive_admission_notional`].
pub fn rounded_order_admission_notional(
    rounded_base_notional: Decimal,
    intended_notional: Decimal,
    max_fee_bps: Decimal,
) -> Result<Decimal, BoltV3SubmitAdmissionError> {
    if rounded_base_notional > intended_notional {
        return Err(BoltV3SubmitAdmissionError::RoundedNotionalExceedsIntent {
            rounded_base_notional,
            intended_notional,
        });
    }
    Ok(fee_inclusive_admission_notional(
        rounded_base_notional,
        max_fee_bps,
    ))
}

/// Admission notional for a market-style order — one with NO firm limit price
/// (Market / StopMarket / MarketIfTouched / TrailingStopMarket). Such an order
/// carries no venue-enforced price bound: it can fill anywhere up to the
/// instrument's structural price ceiling. The per-order cap must therefore be
/// checked against that ceiling — the only price the venue physically cannot
/// exceed — never against a reference-price estimate or a configured slippage
/// budget (an estimate is not a bound). Fails CLOSED when the instrument
/// declares no ceiling: an order whose worst-case cash cost cannot be bounded
/// must not be admitted.
pub fn market_style_admission_ceiling_notional(
    price_ceiling: Option<Decimal>,
    order_quantity: Decimal,
) -> Result<Decimal, BoltV3SubmitAdmissionError> {
    let ceiling = price_ceiling.ok_or(BoltV3SubmitAdmissionError::MissingPriceCeiling)?;
    Ok(base_quantity_admission_notional(ceiling, order_quantity))
}

#[derive(Debug, Eq, PartialEq)]
pub enum BoltV3SubmitAdmissionError {
    NotArmed,
    AlreadyArmed,
    SubmitLifecycleDisallowed {
        intent: BoltV3SubmitIntentKind,
    },
    LossGovernorHalted {
        reasons: Vec<LossHaltReason>,
    },
    CountCapExhausted,
    NonPositiveNotional,
    NotionalCapExceeded,
    MissingPriceCeiling,
    RoundedNotionalExceedsIntent {
        rounded_base_notional: Decimal,
        intended_notional: Decimal,
    },
    InvalidCanaryProofClaim,
    PositionSizingRejected {
        reason: BoltV3PositionSizerRejectReason,
    },
    SystemClock {
        reason: String,
    },
    InvalidRiskReducingExitProof,
    EvidenceWriteFailed {
        reason: String,
    },
}

impl std::fmt::Display for BoltV3SubmitAdmissionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotArmed => write!(f, "bolt-v3 submit admission is not armed"),
            Self::AlreadyArmed => write!(f, "bolt-v3 submit admission is already armed"),
            Self::SubmitLifecycleDisallowed { intent } => write!(
                f,
                "bolt-v3 submit admission lifecycle policy disallows {intent:?} submit"
            ),
            Self::LossGovernorHalted { reasons } => {
                let reasons = reasons
                    .iter()
                    .map(|reason| reason.as_str())
                    .collect::<Vec<_>>()
                    .join(",");
                write!(
                    f,
                    "bolt-v3 submit admission loss governor halted: {reasons}"
                )
            }
            Self::CountCapExhausted => {
                write!(f, "bolt-v3 submit admission order count cap is exhausted")
            }
            Self::NonPositiveNotional => {
                write!(f, "bolt-v3 submit admission notional must be positive")
            }
            Self::NotionalCapExceeded => {
                write!(f, "bolt-v3 submit admission notional cap is exceeded")
            }
            Self::PositionSizingRejected { reason } => {
                write!(
                    f,
                    "bolt-v3 submit admission position sizing rejected: {reason:?}"
                )
            }
            Self::SystemClock { reason } => {
                write!(f, "bolt-v3 submit admission system clock failed: {reason}")
            }
            Self::MissingPriceCeiling => write!(
                f,
                "bolt-v3 submit admission refuses a market-style order without a declared instrument price ceiling"
            ),
            Self::RoundedNotionalExceedsIntent {
                rounded_base_notional,
                intended_notional,
            } => write!(
                f,
                "bolt-v3 submit admission rejected: rounded order notional {rounded_base_notional} exceeded operator-intended notional {intended_notional}"
            ),
            Self::InvalidCanaryProofClaim => write!(
                f,
                "bolt-v3 submit admission canary proof claim must be proof_only"
            ),
            Self::InvalidRiskReducingExitProof => write!(
                f,
                "bolt-v3 submit admission risk-reducing exit proof is invalid"
            ),
            Self::EvidenceWriteFailed { reason } => {
                write!(
                    f,
                    "bolt-v3 submit admission failed to record decision evidence: {reason}"
                )
            }
        }
    }
}

fn lock_inner(
    inner: &Arc<Mutex<BoltV3SubmitAdmissionInner>>,
) -> std::sync::MutexGuard<'_, BoltV3SubmitAdmissionInner> {
    inner
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn evaluate_position_sizer_submit(
    inner: &mut BoltV3SubmitAdmissionInner,
    request: &BoltV3SubmitAdmissionRequest,
    now_ns: u64,
) -> BoltV3PositionSizerSubmitDecision {
    let Some(position_sizer) = inner.position_sizer.as_mut() else {
        return accepted_without_reservation();
    };
    if request.intent_kind == BoltV3SubmitIntentKind::ReplaceSubmit {
        return rejected_position_sizer(BoltV3PositionSizerRejectReason::ReplaceSubmitUnsupported);
    }
    let Some(evidence) = request.position_sizing.as_ref() else {
        return rejected_position_sizer(BoltV3PositionSizerRejectReason::MissingSizingEvidence);
    };
    if evidence.venue_id != position_sizer.venue_id {
        return rejected_position_sizer(BoltV3PositionSizerRejectReason::VenueMismatch);
    }
    let product_kind = evidence.product_kind.to_position_sizer();
    if product_kind != position_sizer.product_kind {
        return rejected_position_sizer(BoltV3PositionSizerRejectReason::ProductKindMismatch);
    }
    if product_kind != ProductKind::PredictionMarketBinary {
        return rejected_position_sizer(BoltV3PositionSizerRejectReason::UnsupportedProductKind);
    }
    if position_sizer
        .client_order_reservations
        .contains_key(&request.client_order_id)
    {
        return rejected_position_sizer(BoltV3PositionSizerRejectReason::DuplicateClientOrderId);
    }
    let Some(state) = position_sizer.state.as_ref() else {
        return rejected_position_sizer(BoltV3PositionSizerRejectReason::MissingNtState);
    };
    if state.portfolio.venue_id != position_sizer.venue_id {
        return rejected_position_sizer(BoltV3PositionSizerRejectReason::VenueMismatch);
    }
    if state.portfolio.account_id != position_sizer.account_id {
        return rejected_position_sizer(BoltV3PositionSizerRejectReason::AccountMismatch);
    }
    if state.portfolio.collateral_currency != position_sizer.collateral_currency {
        return rejected_position_sizer(
            BoltV3PositionSizerRejectReason::CollateralCurrencyMismatch,
        );
    }
    let ProductSizingSnapshot::PredictionMarketBinary(product) = &state.product_state;
    let Some(outcome) = evidence.prediction_market_outcome else {
        return rejected_position_sizer(
            BoltV3PositionSizerRejectReason::MissingPredictionMarketOutcome,
        );
    };
    let outcome_position = match outcome {
        PredictionMarketOutcomeSide::Yes => {
            if request.instrument_id != product.yes_instrument_id {
                return rejected_position_sizer(
                    BoltV3PositionSizerRejectReason::OutcomeInstrumentMismatch,
                );
            }
            product.yes_position
        }
        PredictionMarketOutcomeSide::No => {
            if request.instrument_id != product.no_instrument_id {
                return rejected_position_sizer(
                    BoltV3PositionSizerRejectReason::OutcomeInstrumentMismatch,
                );
            }
            product.no_position
        }
    };

    if request.intent_kind == BoltV3SubmitIntentKind::RiskReducingExit {
        if evidence.side == BoltV3CompiledOrderSide::Sell && evidence.quantity <= outcome_position {
            return accepted_without_reservation();
        }
        return rejected_position_sizer(BoltV3PositionSizerRejectReason::SizingRejected);
    }

    position_sizer.next_sequence += 1;
    let submit_reservation_id = format!(
        "{}#{}",
        request.client_order_id, position_sizer.next_sequence
    );
    let sizing_request = PositionSizingRequest {
        intent_id: submit_reservation_id.clone(),
        strategy_id: request.strategy_id.clone(),
        instrument_id: request.instrument_id.clone(),
        pool_id: position_sizer.capital_pool.pool_id.clone(),
        product_kind,
        side: evidence.side.to_position_sizer(),
        quantity: evidence.quantity,
        limit_price: evidence.effective_price,
        order_kind: evidence.order_kind.to_position_sizer(),
        liquidity: evidence.liquidity.to_position_sizer(),
        quote_set_id: evidence.quote_set_id.clone(),
        now_ns,
    };
    let decision = position_sizer
        .gate
        .evaluate_and_reserve(PositionSizingGateInputs {
            request: &sizing_request,
            state: Some(state),
            policy: &position_sizer.policy,
            loss_policy: None,
            capital_pool: &position_sizer.capital_pool,
        });
    if !decision.accepted {
        return rejected_position_sizer(map_sized_rejection(&decision.reasons));
    }
    if decision.sized_quantity != Some(evidence.quantity) {
        position_sizer.gate.rollback_uncommitted_reservation(
            &position_sizer.capital_pool.pool_id,
            &submit_reservation_id,
        );
        return rejected_position_sizer(BoltV3PositionSizerRejectReason::SizedQuantityMismatch);
    }
    position_sizer.client_order_reservations.insert(
        request.client_order_id.clone(),
        BoltV3SubmitReservationIndex {
            submit_reservation_id: submit_reservation_id.clone(),
            collateral_group_id: product.collateral_coupled_group_id.clone(),
        },
    );
    BoltV3PositionSizerSubmitDecision {
        accepted: true,
        reason: BoltV3PositionSizerRejectReason::Rejected,
        rollback: Some(BoltV3PositionSizerReservationRollback {
            client_order_id: request.client_order_id.clone(),
            submit_reservation_id,
            pool_id: position_sizer.capital_pool.pool_id.clone(),
        }),
    }
}

fn accepted_without_reservation() -> BoltV3PositionSizerSubmitDecision {
    BoltV3PositionSizerSubmitDecision {
        accepted: true,
        reason: BoltV3PositionSizerRejectReason::Rejected,
        rollback: None,
    }
}

fn rejected_position_sizer(
    reason: BoltV3PositionSizerRejectReason,
) -> BoltV3PositionSizerSubmitDecision {
    BoltV3PositionSizerSubmitDecision {
        accepted: false,
        reason,
        rollback: None,
    }
}

fn rollback_position_sizer_reservation(
    inner: &mut BoltV3SubmitAdmissionInner,
    rollback: &BoltV3PositionSizerReservationRollback,
) {
    let Some(position_sizer) = inner.position_sizer.as_mut() else {
        return;
    };
    position_sizer
        .gate
        .rollback_uncommitted_reservation(&rollback.pool_id, &rollback.submit_reservation_id);
    if position_sizer
        .client_order_reservations
        .get(&rollback.client_order_id)
        .map(|current| current.submit_reservation_id.as_str())
        == Some(rollback.submit_reservation_id.as_str())
    {
        position_sizer
            .client_order_reservations
            .remove(&rollback.client_order_id);
    }
}

fn map_sized_rejection(
    reasons: &[crate::bolt_v3_position_sizer::SizedAdmissionReason],
) -> BoltV3PositionSizerRejectReason {
    if reasons.iter().any(|reason| {
        matches!(
            reason,
            crate::bolt_v3_position_sizer::SizedAdmissionReason::MissingNtState
        )
    }) {
        return BoltV3PositionSizerRejectReason::MissingNtState;
    }
    if reasons.iter().any(|reason| {
        matches!(
            reason,
            crate::bolt_v3_position_sizer::SizedAdmissionReason::StaleNtState(_)
        )
    }) {
        return BoltV3PositionSizerRejectReason::StaleNtState;
    }
    if reasons.iter().any(|reason| {
        matches!(
            reason,
            crate::bolt_v3_position_sizer::SizedAdmissionReason::UnattributedNtState(_)
        )
    }) {
        return BoltV3PositionSizerRejectReason::UnattributedNtState;
    }
    if reasons.iter().any(|reason| {
        matches!(
            reason,
            crate::bolt_v3_position_sizer::SizedAdmissionReason::Reservation(
                crate::bolt_v3_capital_reservation::ReservationRejectionReason::ReconciliationRequired,
            )
        )
    }) {
        return BoltV3PositionSizerRejectReason::ReconciliationRequired;
    }
    if reasons.iter().any(|reason| {
        matches!(
            reason,
            crate::bolt_v3_position_sizer::SizedAdmissionReason::Reservation(
                crate::bolt_v3_capital_reservation::ReservationRejectionReason::OverBudget,
            ) | crate::bolt_v3_position_sizer::SizedAdmissionReason::OverMaxOrderLiability
        )
    }) {
        return BoltV3PositionSizerRejectReason::OverBudget;
    }
    BoltV3PositionSizerRejectReason::SizingRejected
}

fn current_unix_ns() -> Result<u64, BoltV3SubmitAdmissionError> {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|source| BoltV3SubmitAdmissionError::SystemClock {
            reason: format!("system time before UNIX_EPOCH: {source}"),
        })?
        .as_nanos();
    nanos
        .try_into()
        .map_err(|_| BoltV3SubmitAdmissionError::SystemClock {
            reason: format!("unix nanoseconds does not fit u64: {nanos}"),
        })
}

impl std::error::Error for BoltV3SubmitAdmissionError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bolt_v3_decision_evidence::{
        BoltV3OrderIntentEvidence, BoltV3StrategyInputEvidenceSnapshot,
    };
    use crate::bolt_v3_position_sizer::{
        FeeSlippagePolicy, PredictionMarketSizingSnapshot, SizingMode,
    };
    use crate::bolt_v3_sizing_state::{
        OrderLifecycleSizingSnapshot, PortfolioSizingSnapshot, ReservationLedgerSnapshot,
    };
    use anyhow::Result;
    use std::{
        sync::mpsc,
        time::{Duration, Instant},
    };

    #[test]
    fn uncommitted_position_sizer_permit_waits_for_lock_and_rolls_back() {
        let admission = position_sized_admission();
        admission
            .arm(BoltV3LiveCanaryGateReport::for_test(
                10,
                Decimal::new(10, 0),
            ))
            .expect("valid gate report should arm admission");
        admission.update_position_sizing_state(fresh_sizing_state(900));
        let rebuild = admission.rebuild_position_sizing_open_order_reservations(Vec::new(), 1_000);
        assert!(rebuild.accepted);

        let permit = admission
            .admit_at(&sized_submit_request("client-order-1"), 1_000)
            .expect("fresh sizing state should admit");
        assert_eq!(
            admission.position_sizer_live_reserved_liability(),
            Some(Decimal::new(43, 1))
        );

        let guard = admission
            .inner
            .lock()
            .expect("test should hold the admission lock");
        let (finished_tx, finished_rx) = mpsc::channel();
        let started_at = Instant::now();
        let drop_thread = std::thread::spawn(move || {
            drop(permit);
            finished_tx
                .send(())
                .expect("drop completion should be observable");
        });

        assert!(
            finished_rx.recv_timeout(Duration::from_millis(50)).is_err(),
            "permit drop should wait for the admission lock instead of skipping rollback"
        );
        drop(guard);
        finished_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("permit drop should finish after the admission lock is released");
        drop_thread
            .join()
            .expect("permit drop thread should not panic");

        assert!(
            started_at.elapsed() >= Duration::from_millis(50),
            "test should prove the rollback path waited on lock contention"
        );
        assert_eq!(
            admission.position_sizer_live_reserved_liability(),
            Some(Decimal::ZERO)
        );
    }

    #[derive(Debug, Default)]
    struct NoopDecisionEvidenceWriter;

    impl BoltV3DecisionEvidenceWriter for NoopDecisionEvidenceWriter {
        fn record_strategy_input_snapshot(
            &self,
            _snapshot: &BoltV3StrategyInputEvidenceSnapshot,
        ) -> Result<()> {
            Ok(())
        }

        fn record_order_intent(&self, _intent: &BoltV3OrderIntentEvidence) -> Result<()> {
            Ok(())
        }

        fn record_admission_decision(
            &self,
            _decision: &BoltV3AdmissionDecisionEvidence,
        ) -> Result<()> {
            Ok(())
        }
    }

    fn position_sized_admission() -> BoltV3SubmitAdmissionState {
        BoltV3SubmitAdmissionState::new_unarmed_with_position_sizer(
            Arc::new(NoopDecisionEvidenceWriter),
            BoltV3SubmitPositionSizerConfig {
                venue_id: "VENUE-A".to_string(),
                account_id: "ACCOUNT-A".to_string(),
                product_kind: ProductKind::PredictionMarketBinary,
                collateral_currency: "PUSD".to_string(),
                capital_pool: CapitalPoolSnapshot {
                    source: "bolt_submit_sizer_bootstrap".to_string(),
                    observed_at_ns: 900,
                    pool_id: "pool-1".to_string(),
                    max_pool_liability: Decimal::new(10, 0),
                    committed_liability: Decimal::ZERO,
                    max_snapshot_age_ns: 500,
                },
                policy: SizingPolicy {
                    mode: SizingMode::RejectOnly,
                    max_order_liability: Some(Decimal::new(10, 0)),
                    min_remaining_pool_balance: None,
                    fee_slippage_policy: Some(FeeSlippagePolicy {
                        max_fee_liability: Decimal::new(10, 2),
                        max_slippage_liability: Decimal::new(20, 2),
                    }),
                },
            },
        )
    }

    fn sized_submit_request(client_order_id: &str) -> BoltV3SubmitAdmissionRequest {
        BoltV3SubmitAdmissionRequest {
            strategy_id: "strategy-a".to_string(),
            client_order_id: client_order_id.to_string(),
            instrument_id: "instrument-yes.VENUE-A".to_string(),
            notional: Decimal::new(4, 0),
            intent_kind: BoltV3SubmitIntentKind::Entry,
            lifecycle_policy: BoltV3SubmitLifecyclePolicy::new(true),
            position_sizing: Some(BoltV3CompiledOrderSizingEvidence {
                venue_id: "VENUE-A".to_string(),
                product_kind: BoltV3CompiledProductKind::PredictionMarketBinary,
                side: BoltV3CompiledOrderSide::Buy,
                quantity: Decimal::new(10, 0),
                effective_price: Decimal::new(40, 2),
                order_kind: BoltV3CompiledOrderKind::Limit,
                liquidity: BoltV3CompiledOrderLiquidity::Taker,
                quote_set_id: None,
                prediction_market_outcome: Some(PredictionMarketOutcomeSide::Yes),
            }),
        }
    }

    fn fresh_sizing_state(observed_at_ns: u64) -> NtDerivedSizingState {
        NtDerivedSizingState {
            source: "nt_sizing_state".to_string(),
            observed_at_ns,
            portfolio: PortfolioSizingSnapshot {
                source: "nt_portfolio_snapshot".to_string(),
                observed_at_ns,
                venue_id: "VENUE-A".to_string(),
                account_id: "ACCOUNT-A".to_string(),
                collateral_currency: "PUSD".to_string(),
                free_collateral: Decimal::new(100, 0),
                total_equity: Decimal::new(100, 0),
            },
            order_lifecycle: OrderLifecycleSizingSnapshot {
                source: "nt_open_order_cache".to_string(),
                observed_at_ns,
                open_order_count: 0,
                all_open_orders_attributed: true,
            },
            product_state: ProductSizingSnapshot::PredictionMarketBinary(
                PredictionMarketSizingSnapshot {
                    source: "nt_prediction_market_snapshot".to_string(),
                    observed_at_ns,
                    yes_instrument_id: "instrument-yes.VENUE-A".to_string(),
                    no_instrument_id: "instrument-no.VENUE-A".to_string(),
                    yes_position: Decimal::new(10, 0),
                    no_position: Decimal::ZERO,
                    pusd_allowance: Decimal::new(100, 0),
                    conditional_token_allowance: Decimal::new(10, 0),
                    collateral_coupled_group_id: "group-1".to_string(),
                },
            ),
            reservation_snapshot: ReservationLedgerSnapshot {
                source: "bolt_reservation_ledger".to_string(),
                observed_at_ns,
                all_live_reservations_attributed: true,
            },
            loss_snapshot: None,
        }
    }
}
