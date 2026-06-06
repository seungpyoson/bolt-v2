use crate::bolt_v3_decision_evidence::{
    BoltV3AdmissionDecisionEvidence, BoltV3AdmissionOutcome, BoltV3DecisionEvidenceWriter,
    BoltV3OrderIntentEvidence, BoltV3OrderIntentKind, compiled_order_price_source,
};
use crate::bolt_v3_kill_switch::{KillSwitchState, KillSwitchStateKind};
use crate::bolt_v3_live_canary_gate::BoltV3LiveCanaryGateReport;
use anyhow::Context;
use nautilus_model::{
    enums::{OrderSide, PositionSide},
    instruments::{Instrument, InstrumentAny},
    orders::{Order, OrderAny},
    types::Price,
};
use rust_decimal::Decimal;
use std::collections::BTreeMap;
use std::str::FromStr;
use std::sync::{Arc, Mutex};

use crate::bolt_v3_canary_proof_policy::CANARY_PROOF_CLAIM;
pub use crate::bolt_v3_decision_evidence::BoltV3SubmitIntentKind;

const SUBMIT_ADMISSION_BPS_DENOMINATOR: u32 = 10_000;

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct BoltV3ExchangeMutationCounts {
    pub submit: u64,
    pub cancel: u64,
    pub modify: u64,
    pub transfer: u64,
    pub account: u64,
}

impl BoltV3ExchangeMutationCounts {
    pub const fn none() -> Self {
        Self {
            submit: 0,
            cancel: 0,
            modify: 0,
            transfer: 0,
            account: 0,
        }
    }

    pub fn total(self) -> Result<u64, BoltV3SubmitAdmissionError> {
        self.submit
            .checked_add(self.cancel)
            .and_then(|total| total.checked_add(self.modify))
            .and_then(|total| total.checked_add(self.transfer))
            .and_then(|total| total.checked_add(self.account))
            .ok_or(BoltV3SubmitAdmissionError::ExchangeMutationCountOverflow)
    }
}

pub fn validate_no_exchange_mutations(
    counts: BoltV3ExchangeMutationCounts,
) -> Result<u64, BoltV3SubmitAdmissionError> {
    let mutation_count = counts.total()?;
    if mutation_count == 0 {
        return Ok(mutation_count);
    }
    Err(BoltV3SubmitAdmissionError::ExchangeMutationsObserved { mutation_count })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoltV3LiveSubmitApprovalLimits {
    pub max_order_count: u32,
    pub max_order_notional: Decimal,
}

#[derive(Debug)]
pub struct BoltV3SubmitAdmissionState {
    inner: Mutex<BoltV3SubmitAdmissionInner>,
    decision_evidence: Arc<dyn BoltV3DecisionEvidenceWriter>,
}

#[derive(Debug)]
struct BoltV3SubmitAdmissionInner {
    gate_report: Option<BoltV3LiveCanaryGateReport>,
    kill_switch_state: KillSwitchState,
    kill_switch_forced_reduction_policy: Option<BoltV3KillSwitchForcedReductionPolicy>,
    live_submit_approval_limits: BTreeMap<String, BoltV3LiveSubmitApprovalLimits>,
    admitted_order_count: u32,
    admitted_order_count_by_execution_client: BTreeMap<String, u32>,
    admitted_entry_order_count: u32,
    admitted_risk_reducing_exit_order_count: u32,
    admitted_replace_submit_order_count: u32,
    live_kill_switch_forced_reduction_order_count: u32,
}

impl BoltV3SubmitAdmissionState {
    pub fn new_unarmed(decision_evidence: Arc<dyn BoltV3DecisionEvidenceWriter>) -> Self {
        Self::new_unarmed_with_live_submit_limits(decision_evidence, BTreeMap::new())
    }

    pub fn new_unarmed_with_live_submit_limits(
        decision_evidence: Arc<dyn BoltV3DecisionEvidenceWriter>,
        live_submit_approval_limits: BTreeMap<String, BoltV3LiveSubmitApprovalLimits>,
    ) -> Self {
        Self {
            inner: Mutex::new(BoltV3SubmitAdmissionInner {
                gate_report: None,
                kill_switch_state: KillSwitchState::Armed,
                kill_switch_forced_reduction_policy: None,
                live_submit_approval_limits,
                admitted_order_count: 0,
                admitted_order_count_by_execution_client: BTreeMap::new(),
                admitted_entry_order_count: 0,
                admitted_risk_reducing_exit_order_count: 0,
                admitted_replace_submit_order_count: 0,
                live_kill_switch_forced_reduction_order_count: 0,
            }),
            decision_evidence,
        }
    }

    pub fn arm(
        &self,
        report: BoltV3LiveCanaryGateReport,
    ) -> Result<(), BoltV3SubmitAdmissionError> {
        let mut inner = self
            .inner
            .lock()
            .expect("submit admission state mutex should not be poisoned");
        if inner.gate_report.is_some() {
            return Err(BoltV3SubmitAdmissionError::AlreadyArmed);
        }
        inner.gate_report = Some(report);
        inner.admitted_order_count = 0;
        inner.admitted_order_count_by_execution_client.clear();
        inner.admitted_entry_order_count = 0;
        inner.admitted_risk_reducing_exit_order_count = 0;
        inner.admitted_replace_submit_order_count = 0;
        inner.live_kill_switch_forced_reduction_order_count = 0;
        Ok(())
    }

    pub fn record_kill_switch_forced_reduction_terminal(&self) {
        let mut inner = self
            .inner
            .lock()
            .expect("submit admission state mutex should not be poisoned");
        inner.live_kill_switch_forced_reduction_order_count = inner
            .live_kill_switch_forced_reduction_order_count
            .saturating_sub(1);
    }

    pub fn replace_kill_switch_state(&self, state: KillSwitchState) {
        let mut inner = self
            .inner
            .lock()
            .expect("submit admission state mutex should not be poisoned");
        inner.kill_switch_state = state;
    }

    pub fn configure_kill_switch_forced_reduction_policy(
        &self,
        policy: BoltV3KillSwitchForcedReductionPolicy,
    ) {
        let mut inner = self
            .inner
            .lock()
            .expect("submit admission state mutex should not be poisoned");
        inner.kill_switch_forced_reduction_policy = Some(policy);
    }

    pub fn admit(
        &self,
        request: &BoltV3SubmitAdmissionRequest,
    ) -> Result<BoltV3SubmitAdmissionPermit, BoltV3SubmitAdmissionError> {
        let mut inner = self
            .inner
            .lock()
            .expect("submit admission state mutex should not be poisoned");
        let outcome = Self::evaluate(&inner, request);
        let evidence = BoltV3AdmissionDecisionEvidence {
            strategy_id: request.strategy_id.clone(),
            execution_client_id: request.execution_client_id.clone(),
            client_order_id: request.client_order_id.clone(),
            instrument_id: request.instrument_id.clone(),
            notional: request.notional.to_string(),
            intent_kind: request.intent_kind,
            outcome: outcome.clone(),
        };
        self.decision_evidence
            .record_admission_decision(&evidence)
            .map_err(|err| BoltV3SubmitAdmissionError::EvidenceWriteFailed {
                reason: format!("{err:#}"),
            })?;
        match outcome {
            BoltV3AdmissionOutcome::Admitted => {
                inner.admitted_order_count += 1;
                *inner
                    .admitted_order_count_by_execution_client
                    .entry(request.execution_client_id.clone())
                    .or_insert(0) += 1;
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
                    BoltV3SubmitIntentKind::KillSwitchForcedReduction => {
                        inner.live_kill_switch_forced_reduction_order_count += 1;
                    }
                }
                Ok(BoltV3SubmitAdmissionPermit(()))
            }
            BoltV3AdmissionOutcome::RejectedNotArmed => Err(BoltV3SubmitAdmissionError::NotArmed),
            BoltV3AdmissionOutcome::RejectedKillSwitchLatched => {
                Err(BoltV3SubmitAdmissionError::KillSwitchLatched {
                    state: inner.kill_switch_state.kind(),
                })
            }
            BoltV3AdmissionOutcome::RejectedSubmitLifecycleDisallowed => {
                Err(BoltV3SubmitAdmissionError::SubmitLifecycleDisallowed {
                    intent: request.intent_kind,
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
            BoltV3AdmissionOutcome::RejectedKillSwitchForcedReductionProofInvalid => {
                Err(BoltV3SubmitAdmissionError::KillSwitchForcedReductionProofInvalid)
            }
            BoltV3AdmissionOutcome::RejectedKillSwitchForcedReductionCapExceeded => {
                Err(BoltV3SubmitAdmissionError::KillSwitchForcedReductionCapExceeded)
            }
        }
    }

    fn evaluate(
        inner: &BoltV3SubmitAdmissionInner,
        request: &BoltV3SubmitAdmissionRequest,
    ) -> BoltV3AdmissionOutcome {
        if request.intent_kind == BoltV3SubmitIntentKind::KillSwitchForcedReduction {
            return Self::evaluate_kill_switch_forced_reduction(inner, request);
        }
        if matches!(
            request.intent_kind,
            BoltV3SubmitIntentKind::Entry | BoltV3SubmitIntentKind::ReplaceSubmit
        ) && inner.kill_switch_state.kind() != KillSwitchStateKind::Armed
        {
            return BoltV3AdmissionOutcome::RejectedKillSwitchLatched;
        }
        if !request.lifecycle_policy.allows(request.intent_kind) {
            return BoltV3AdmissionOutcome::RejectedSubmitLifecycleDisallowed;
        }
        if request.notional <= Decimal::ZERO {
            return BoltV3AdmissionOutcome::RejectedNonPositiveNotional;
        }
        if let Some(limits) = inner
            .live_submit_approval_limits
            .get(&request.execution_client_id)
        {
            if request.notional > limits.max_order_notional {
                return BoltV3AdmissionOutcome::RejectedNotionalCapExceeded;
            }
            if inner
                .admitted_order_count_by_execution_client
                .get(&request.execution_client_id)
                .copied()
                .unwrap_or(0)
                >= limits.max_order_count
            {
                return BoltV3AdmissionOutcome::RejectedCountCapExhausted;
            }
        }
        let Some(report) = inner.gate_report.as_ref() else {
            if request.canary_proof_claim.is_some() {
                return BoltV3AdmissionOutcome::RejectedInvalidCanaryProofClaim;
            }
            return BoltV3AdmissionOutcome::Admitted;
        };

        if matches!(
            request.intent_kind,
            BoltV3SubmitIntentKind::Entry | BoltV3SubmitIntentKind::ReplaceSubmit
        ) && request.notional > report.max_notional_per_order()
        {
            return BoltV3AdmissionOutcome::RejectedNotionalCapExceeded;
        }
        if request
            .canary_proof_claim
            .as_deref()
            .is_some_and(|claim| claim != CANARY_PROOF_CLAIM)
        {
            return BoltV3AdmissionOutcome::RejectedInvalidCanaryProofClaim;
        }
        match request.intent_kind {
            BoltV3SubmitIntentKind::Entry => {
                if inner.admitted_entry_order_count >= report.max_live_entry_order_count() {
                    return BoltV3AdmissionOutcome::RejectedCountCapExhausted;
                }
            }
            BoltV3SubmitIntentKind::RiskReducingExit => {
                let Some(proof) = request.risk_reducing_exit_proof.as_ref() else {
                    return BoltV3AdmissionOutcome::RejectedInvalidRiskReducingExitProof;
                };
                if !proof.is_valid_for(request) {
                    return BoltV3AdmissionOutcome::RejectedInvalidRiskReducingExitProof;
                }
                if inner.admitted_risk_reducing_exit_order_count
                    >= report.max_live_risk_reducing_exit_order_count()
                {
                    return BoltV3AdmissionOutcome::RejectedCountCapExhausted;
                }
            }
            BoltV3SubmitIntentKind::ReplaceSubmit => {
                if inner.admitted_replace_submit_order_count
                    >= report.max_live_replace_submit_order_count()
                {
                    return BoltV3AdmissionOutcome::RejectedCountCapExhausted;
                }
            }
            BoltV3SubmitIntentKind::KillSwitchForcedReduction => {
                unreachable!("kill-switch forced reduction is evaluated before normal admission")
            }
        }
        BoltV3AdmissionOutcome::Admitted
    }

    fn evaluate_kill_switch_forced_reduction(
        inner: &BoltV3SubmitAdmissionInner,
        request: &BoltV3SubmitAdmissionRequest,
    ) -> BoltV3AdmissionOutcome {
        let Some(policy) = inner.kill_switch_forced_reduction_policy.as_ref() else {
            return BoltV3AdmissionOutcome::RejectedKillSwitchForcedReductionProofInvalid;
        };
        let Some(claim) = request.kill_switch_forced_reduction.as_ref() else {
            return BoltV3AdmissionOutcome::RejectedKillSwitchForcedReductionProofInvalid;
        };
        let Some(halt_id) = forced_reduction_admissible_halt_id(&inner.kill_switch_state) else {
            return BoltV3AdmissionOutcome::RejectedKillSwitchForcedReductionProofInvalid;
        };
        if claim.halt_id() != halt_id || claim.policy_sha256() != policy.policy_sha256() {
            return BoltV3AdmissionOutcome::RejectedKillSwitchForcedReductionProofInvalid;
        }
        if request.notional <= Decimal::ZERO {
            return BoltV3AdmissionOutcome::RejectedNonPositiveNotional;
        }
        if request.notional > policy.max_notional_per_order() {
            return BoltV3AdmissionOutcome::RejectedKillSwitchForcedReductionCapExceeded;
        }
        if inner.live_kill_switch_forced_reduction_order_count >= policy.max_live_order_count() {
            return BoltV3AdmissionOutcome::RejectedKillSwitchForcedReductionCapExceeded;
        }
        BoltV3AdmissionOutcome::Admitted
    }

    pub fn admitted_order_count(&self) -> u32 {
        self.inner
            .lock()
            .expect("submit admission state mutex should not be poisoned")
            .admitted_order_count
    }

    /// Gate-approved maximum reference-quote age (seconds) carried by the armed
    /// gate report, or `None` when the state is not yet armed. This is the single
    /// authoritative freshness bound when the optional gate is armed (A5): the
    /// submit / forced-flat stale check plumbs this value in so the gate-validated
    /// freshness policy — not an independent strategy-config value — governs
    /// whether a reference quote is fresh enough to keep trading.
    pub fn reference_quote_max_age_seconds(&self) -> Option<u64> {
        self.inner
            .lock()
            .expect("submit admission state mutex should not be poisoned")
            .gate_report
            .as_ref()
            .map(BoltV3LiveCanaryGateReport::reference_quote_max_age_seconds)
    }
}

#[derive(Debug)]
pub struct BoltV3SubmitAdmissionPermit(());

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoltV3KillSwitchForcedReductionPolicy {
    policy_sha256: String,
    max_live_order_count: u32,
    max_notional_per_order: Decimal,
}

impl BoltV3KillSwitchForcedReductionPolicy {
    pub fn new(
        policy_sha256: impl Into<String>,
        max_live_order_count: u32,
        max_notional_per_order: Decimal,
    ) -> Result<Self, BoltV3KillSwitchForcedReductionError> {
        let policy_sha256 = policy_sha256.into();
        if policy_sha256.len() != 64 || !policy_sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(BoltV3KillSwitchForcedReductionError::InvalidPolicySha256);
        }
        if max_live_order_count == 0 {
            return Err(BoltV3KillSwitchForcedReductionError::NonPositiveMaxLiveOrderCount);
        }
        if max_notional_per_order <= Decimal::ZERO {
            return Err(BoltV3KillSwitchForcedReductionError::NonPositiveMaxNotional);
        }
        Ok(Self {
            policy_sha256,
            max_live_order_count,
            max_notional_per_order,
        })
    }

    pub fn policy_sha256(&self) -> &str {
        &self.policy_sha256
    }

    pub fn max_live_order_count(&self) -> u32 {
        self.max_live_order_count
    }

    pub fn max_notional_per_order(&self) -> Decimal {
        self.max_notional_per_order
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoltV3KillSwitchForcedReductionClaim {
    halt_id: String,
    action_id: String,
    policy_sha256: String,
}

impl BoltV3KillSwitchForcedReductionClaim {
    pub fn new(
        halt_id: impl Into<String>,
        action_id: impl Into<String>,
        policy_sha256: impl Into<String>,
    ) -> Result<Self, BoltV3KillSwitchForcedReductionError> {
        let halt_id = halt_id.into();
        let action_id = action_id.into();
        let policy_sha256 = policy_sha256.into();
        if halt_id.trim().is_empty() {
            return Err(BoltV3KillSwitchForcedReductionError::MissingHaltId);
        }
        if action_id.trim().is_empty() {
            return Err(BoltV3KillSwitchForcedReductionError::MissingActionId);
        }
        if policy_sha256.len() != 64 || !policy_sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(BoltV3KillSwitchForcedReductionError::InvalidPolicySha256);
        }
        Ok(Self {
            halt_id,
            action_id,
            policy_sha256,
        })
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoltV3KillSwitchForcedReductionError {
    MissingHaltId,
    MissingActionId,
    InvalidPolicySha256,
    NonPositiveMaxLiveOrderCount,
    NonPositiveMaxNotional,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoltV3RiskReducingExitPositionInput {
    pub position_id: String,
    pub instrument_id: String,
    pub position_side: PositionSide,
    pub position_quantity: Decimal,
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
            BoltV3SubmitIntentKind::KillSwitchForcedReduction => true,
        }
    }
}

#[derive(Debug)]
pub struct BoltV3SubmitAdmissionRequest {
    pub strategy_id: String,
    pub execution_client_id: String,
    pub client_order_id: String,
    pub instrument_id: String,
    pub notional: Decimal,
    pub order_side: OrderSide,
    pub order_quantity: Decimal,
    pub intent_kind: BoltV3SubmitIntentKind,
    pub lifecycle_policy: BoltV3SubmitLifecyclePolicy,
    pub canary_proof_claim: Option<String>,
    pub risk_reducing_exit_proof: Option<BoltV3RiskReducingExitProof>,
    pub kill_switch_forced_reduction: Option<BoltV3KillSwitchForcedReductionClaim>,
}

#[derive(Debug, Clone)]
pub struct BoltV3SubmitAdmissionRequestInput<'a> {
    pub execution_client_id: &'a str,
    pub intent: &'a BoltV3OrderIntentEvidence,
    pub order: &'a OrderAny,
    pub instrument: Option<&'a InstrumentAny>,
    pub quote_quantity_last_price: Option<Price>,
    pub quote_quantity_reference_price: Option<Price>,
    pub lifecycle_policy: BoltV3SubmitLifecyclePolicy,
    pub risk_reducing_exit_position: Option<BoltV3RiskReducingExitPositionInput>,
}

pub fn build_submit_admission_request_from_order<F>(
    input: BoltV3SubmitAdmissionRequestInput<'_>,
    max_fee_bps_for_price: F,
) -> anyhow::Result<BoltV3SubmitAdmissionRequest>
where
    F: FnOnce(Decimal) -> anyhow::Result<Decimal>,
{
    let client_order_id = input.order.client_order_id().to_string();
    let quantity_source = input.order.quantity().to_string();
    let quantity = Decimal::from_str(quantity_source.trim()).with_context(|| {
        format!(
            "bolt-v3 submit admission quantity is not a decimal for client_order_id={}",
            client_order_id
        )
    })?;
    let price_source = compiled_order_price_source(input.intent.price.clone(), input.order);
    let price = Decimal::from_str(price_source.trim()).with_context(|| {
        format!(
            "bolt-v3 submit admission price is not a decimal for client_order_id={}",
            client_order_id
        )
    })?;
    let notional = if input.order.is_quote_quantity() {
        let instrument = input.instrument.with_context(|| {
            format!(
                "bolt-v3 submit admission missing instrument context for quote-quantity client_order_id={}",
                client_order_id
            )
        })?;
        match admission_base_notional_from_order(
            input.order,
            instrument,
            price,
            quantity,
            input.quote_quantity_last_price,
            input.quote_quantity_reference_price,
        ) {
            Some(base_notional) => base_notional,
            None => {
                anyhow::ensure!(
                    !instrument.is_inverse(),
                    "bolt-v3 submit admission cannot value a quote-quantity order on an inverse instrument from the raw quote quantity (client_order_id={})",
                    client_order_id
                );
                quantity
            }
        }
    } else {
        base_quantity_admission_notional(price, quantity)
    };
    let notional = if input.order.price().is_none() && !input.order.is_quote_quantity() {
        let price_ceiling = input
            .instrument
            .and_then(|instrument| instrument.max_price())
            .map(|ceiling| ceiling.as_decimal());
        market_style_admission_ceiling_notional(price_ceiling, quantity).with_context(|| {
            format!(
                "bolt-v3 submit admission refuses a market-style order without a structural price ceiling for client_order_id={}",
                client_order_id
            )
        })?
    } else {
        notional
    };
    let max_fee_bps = max_fee_bps_for_price(price)?;
    let notional = fee_inclusive_admission_notional(notional, max_fee_bps);
    let intent_kind = match input.intent.intent_kind {
        BoltV3OrderIntentKind::Entry => BoltV3SubmitIntentKind::Entry,
        BoltV3OrderIntentKind::Exit => BoltV3SubmitIntentKind::RiskReducingExit,
    };
    let risk_reducing_exit_proof =
        if matches!(intent_kind, BoltV3SubmitIntentKind::RiskReducingExit) {
            input
                .risk_reducing_exit_position
                .map(|position| BoltV3RiskReducingExitProof {
                    position_id: position.position_id,
                    instrument_id: position.instrument_id,
                    position_side: position.position_side,
                    exit_order_side: input.order.order_side(),
                    position_quantity: position.position_quantity,
                    exit_quantity: quantity,
                })
        } else {
            None
        };

    Ok(BoltV3SubmitAdmissionRequest {
        strategy_id: input.intent.strategy_id.clone(),
        execution_client_id: input.execution_client_id.to_string(),
        client_order_id,
        instrument_id: input.order.instrument_id().to_string(),
        notional,
        order_side: input.order.order_side(),
        order_quantity: quantity,
        intent_kind,
        lifecycle_policy: input.lifecycle_policy,
        canary_proof_claim: None,
        risk_reducing_exit_proof,
        kill_switch_forced_reduction: None,
    })
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

fn forced_reduction_admissible_halt_id(state: &KillSwitchState) -> Option<&str> {
    match state {
        KillSwitchState::Halting { halt_id, .. } | KillSwitchState::Halted { halt_id, .. } => {
            Some(halt_id)
        }
        KillSwitchState::Armed
        | KillSwitchState::Flat { .. }
        | KillSwitchState::FailedManualIntervention { .. } => None,
    }
}

#[derive(Debug, Eq, PartialEq)]
pub enum BoltV3SubmitAdmissionError {
    NotArmed,
    AlreadyArmed,
    KillSwitchLatched {
        state: KillSwitchStateKind,
    },
    SubmitLifecycleDisallowed {
        intent: BoltV3SubmitIntentKind,
    },
    CountCapExhausted,
    NonPositiveNotional,
    NotionalCapExceeded,
    MissingPriceCeiling,
    RoundedNotionalExceedsIntent {
        rounded_base_notional: Decimal,
        intended_notional: Decimal,
    },
    ExchangeMutationCountOverflow,
    ExchangeMutationsObserved {
        mutation_count: u64,
    },
    InvalidCanaryProofClaim,
    InvalidRiskReducingExitProof,
    KillSwitchForcedReductionProofInvalid,
    KillSwitchForcedReductionCapExceeded,
    EvidenceWriteFailed {
        reason: String,
    },
}

impl std::fmt::Display for BoltV3SubmitAdmissionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotArmed => write!(f, "bolt-v3 submit admission is not armed"),
            Self::AlreadyArmed => write!(f, "bolt-v3 submit admission is already armed"),
            Self::KillSwitchLatched { state } => write!(
                f,
                "bolt-v3 submit admission is blocked by kill-switch state {state:?}"
            ),
            Self::SubmitLifecycleDisallowed { intent } => write!(
                f,
                "bolt-v3 submit admission lifecycle policy disallows {intent:?} submit"
            ),
            Self::CountCapExhausted => {
                write!(f, "bolt-v3 submit admission order count cap is exhausted")
            }
            Self::NonPositiveNotional => {
                write!(f, "bolt-v3 submit admission notional must be positive")
            }
            Self::NotionalCapExceeded => {
                write!(f, "bolt-v3 submit admission notional cap is exceeded")
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
            Self::ExchangeMutationCountOverflow => {
                write!(f, "bolt-v3 no-submit exchange mutation counter overflowed")
            }
            Self::ExchangeMutationsObserved { mutation_count } => write!(
                f,
                "bolt-v3 no-submit exchange mutation guard observed {mutation_count} mutating request(s)"
            ),
            Self::InvalidCanaryProofClaim => write!(
                f,
                "bolt-v3 submit admission canary proof claim must be proof_only"
            ),
            Self::InvalidRiskReducingExitProof => write!(
                f,
                "bolt-v3 submit admission risk-reducing exit proof is invalid"
            ),
            Self::KillSwitchForcedReductionProofInvalid => write!(
                f,
                "bolt-v3 submit admission kill-switch forced reduction proof is invalid"
            ),
            Self::KillSwitchForcedReductionCapExceeded => write!(
                f,
                "bolt-v3 submit admission kill-switch forced reduction cap is exceeded"
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

impl std::error::Error for BoltV3SubmitAdmissionError {}
