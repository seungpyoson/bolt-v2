use crate::bolt_v3_decision_evidence::{
    BoltV3AdmissionDecisionEvidence, BoltV3AdmissionOutcome, BoltV3DecisionEvidenceWriter,
};
use crate::bolt_v3_live_canary_gate::BoltV3LiveCanaryGateReport;
use crate::bolt_v3_loss_governor::{LossGovernorPolicy, LossSnapshot, evaluate_loss_admission};
use rust_decimal::Decimal;
use std::{
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

pub use crate::bolt_v3_decision_evidence::BoltV3SubmitIntentKind;

#[derive(Debug)]
pub struct BoltV3SubmitAdmissionState {
    inner: Mutex<BoltV3SubmitAdmissionInner>,
    decision_evidence: Arc<dyn BoltV3DecisionEvidenceWriter>,
}

#[derive(Debug)]
struct BoltV3SubmitAdmissionInner {
    gate_report: Option<BoltV3LiveCanaryGateReport>,
    admitted_order_count: u32,
    admitted_entry_order_count: u32,
    admitted_risk_reducing_exit_order_count: u32,
    loss_governor_policy: Option<LossGovernorPolicy>,
    loss_snapshot: Option<LossSnapshot>,
}

impl BoltV3SubmitAdmissionState {
    pub fn new_unarmed(decision_evidence: Arc<dyn BoltV3DecisionEvidenceWriter>) -> Self {
        Self::new_unarmed_with_optional_loss_governor(decision_evidence, None)
    }

    pub fn new_unarmed_with_loss_governor(
        decision_evidence: Arc<dyn BoltV3DecisionEvidenceWriter>,
        policy: LossGovernorPolicy,
    ) -> Self {
        Self::new_unarmed_with_optional_loss_governor(decision_evidence, Some(policy))
    }

    fn new_unarmed_with_optional_loss_governor(
        decision_evidence: Arc<dyn BoltV3DecisionEvidenceWriter>,
        loss_governor_policy: Option<LossGovernorPolicy>,
    ) -> Self {
        Self {
            inner: Mutex::new(BoltV3SubmitAdmissionInner {
                gate_report: None,
                admitted_order_count: 0,
                admitted_entry_order_count: 0,
                admitted_risk_reducing_exit_order_count: 0,
                loss_governor_policy,
                loss_snapshot: None,
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
        inner.admitted_entry_order_count = 0;
        inner.admitted_risk_reducing_exit_order_count = 0;
        Ok(())
    }

    pub fn update_loss_snapshot(&self, snapshot: LossSnapshot) {
        self.inner
            .lock()
            .expect("submit admission state mutex should not be poisoned")
            .loss_snapshot = Some(snapshot);
    }

    pub fn admit(
        &self,
        request: &BoltV3SubmitAdmissionRequest,
    ) -> Result<BoltV3SubmitAdmissionPermit, BoltV3SubmitAdmissionError> {
        self.admit_at(request, current_unix_nanos())
    }

    pub fn admit_at(
        &self,
        request: &BoltV3SubmitAdmissionRequest,
        now_ns: u64,
    ) -> Result<BoltV3SubmitAdmissionPermit, BoltV3SubmitAdmissionError> {
        let mut inner = self
            .inner
            .lock()
            .expect("submit admission state mutex should not be poisoned");
        let evaluation = Self::evaluate(&inner, request, now_ns);
        let evidence = BoltV3AdmissionDecisionEvidence {
            strategy_id: request.strategy_id.clone(),
            client_order_id: request.client_order_id.clone(),
            instrument_id: request.instrument_id.clone(),
            notional: request.notional.to_string(),
            intent_kind: request.intent_kind,
            outcome: evaluation.outcome.clone(),
            loss_halt_reasons: evaluation.loss_halt_reasons.clone(),
        };
        self.decision_evidence
            .record_admission_decision(&evidence)
            .map_err(|err| BoltV3SubmitAdmissionError::EvidenceWriteFailed {
                reason: format!("{err:#}"),
            })?;
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
                    BoltV3SubmitIntentKind::ReplaceSubmit => {}
                }
                Ok(BoltV3SubmitAdmissionPermit(()))
            }
            BoltV3AdmissionOutcome::RejectedNotArmed => Err(BoltV3SubmitAdmissionError::NotArmed),
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
            BoltV3AdmissionOutcome::RejectedCountCapExhausted => {
                Err(BoltV3SubmitAdmissionError::CountCapExhausted)
            }
            BoltV3AdmissionOutcome::RejectedLossGovernorHalted => {
                Err(BoltV3SubmitAdmissionError::LossGovernorHalted {
                    reasons: evaluation.loss_halt_reasons,
                })
            }
        }
    }

    fn evaluate(
        inner: &BoltV3SubmitAdmissionInner,
        request: &BoltV3SubmitAdmissionRequest,
        now_ns: u64,
    ) -> BoltV3AdmissionEvaluation {
        let Some(report) = inner.gate_report.as_ref() else {
            return BoltV3AdmissionEvaluation::new(BoltV3AdmissionOutcome::RejectedNotArmed);
        };
        if !request.lifecycle_policy.allows(request.intent_kind) {
            return BoltV3AdmissionEvaluation::new(
                BoltV3AdmissionOutcome::RejectedSubmitLifecycleDisallowed,
            );
        }
        if request.notional <= Decimal::ZERO {
            return BoltV3AdmissionEvaluation::new(
                BoltV3AdmissionOutcome::RejectedNonPositiveNotional,
            );
        }
        if request.notional > report.max_notional_per_order() {
            return BoltV3AdmissionEvaluation::new(
                BoltV3AdmissionOutcome::RejectedNotionalCapExceeded,
            );
        }
        if request.intent_kind != BoltV3SubmitIntentKind::RiskReducingExit
            && let Some(policy) = inner.loss_governor_policy.as_ref()
        {
            let decision = evaluate_loss_admission(policy, inner.loss_snapshot.as_ref(), now_ns);
            if !decision.accepted {
                return BoltV3AdmissionEvaluation {
                    outcome: BoltV3AdmissionOutcome::RejectedLossGovernorHalted,
                    loss_halt_reasons: decision
                        .halt_reasons
                        .into_iter()
                        .map(|reason| reason.as_str().to_string())
                        .collect(),
                };
            }
        }
        if inner.admitted_order_count >= report.max_live_order_count() {
            return BoltV3AdmissionEvaluation::new(
                BoltV3AdmissionOutcome::RejectedCountCapExhausted,
            );
        }
        BoltV3AdmissionEvaluation::new(BoltV3AdmissionOutcome::Admitted)
    }

    pub fn admitted_order_count(&self) -> u32 {
        self.inner
            .lock()
            .expect("submit admission state mutex should not be poisoned")
            .admitted_order_count
    }

    pub fn loss_governor_enabled(&self) -> bool {
        self.inner
            .lock()
            .expect("submit admission state mutex should not be poisoned")
            .loss_governor_policy
            .is_some()
    }
}

#[derive(Debug)]
pub struct BoltV3SubmitAdmissionPermit(());

#[derive(Debug)]
struct BoltV3AdmissionEvaluation {
    outcome: BoltV3AdmissionOutcome,
    loss_halt_reasons: Vec<String>,
}

impl BoltV3AdmissionEvaluation {
    fn new(outcome: BoltV3AdmissionOutcome) -> Self {
        Self {
            outcome,
            loss_halt_reasons: Vec::new(),
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
    pub intent_kind: BoltV3SubmitIntentKind,
    pub lifecycle_policy: BoltV3SubmitLifecyclePolicy,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct BoltV3QuoteQuantityAdmissionInput {
    pub order_kind: BoltV3QuoteQuantityOrderKind,
    pub order_side: BoltV3QuoteQuantityOrderSide,
    pub is_quote_quantity: bool,
    pub is_inverse: bool,
    pub submitted_quote_quantity: Decimal,
    pub calculated_notional: Decimal,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum BoltV3QuoteQuantityOrderKind {
    Limit,
    StopLimit,
    Other,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum BoltV3QuoteQuantityOrderSide {
    Buy,
    Sell,
    Other,
}

pub fn conservative_quote_quantity_admission_notional(
    input: BoltV3QuoteQuantityAdmissionInput,
) -> Decimal {
    if input.is_quote_quantity
        && !input.is_inverse
        && input.order_side == BoltV3QuoteQuantityOrderSide::Sell
        && matches!(
            input.order_kind,
            BoltV3QuoteQuantityOrderKind::Limit | BoltV3QuoteQuantityOrderKind::StopLimit
        )
    {
        return input
            .calculated_notional
            .max(input.submitted_quote_quantity);
    }
    input.calculated_notional
}

#[derive(Debug, Eq, PartialEq)]
pub enum BoltV3SubmitAdmissionError {
    NotArmed,
    AlreadyArmed,
    SubmitLifecycleDisallowed { intent: BoltV3SubmitIntentKind },
    CountCapExhausted,
    NonPositiveNotional,
    NotionalCapExceeded,
    LossGovernorHalted { reasons: Vec<String> },
    EvidenceWriteFailed { reason: String },
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
            Self::CountCapExhausted => {
                write!(f, "bolt-v3 submit admission order count cap is exhausted")
            }
            Self::NonPositiveNotional => {
                write!(f, "bolt-v3 submit admission notional must be positive")
            }
            Self::NotionalCapExceeded => {
                write!(f, "bolt-v3 submit admission notional cap is exceeded")
            }
            Self::LossGovernorHalted { reasons } => {
                write!(
                    f,
                    "bolt-v3 submit admission loss governor halted new risk: {}",
                    reasons.join(",")
                )
            }
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

fn current_unix_nanos() -> u64 {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time should not be before unix epoch")
        .as_nanos();
    nanos
        .try_into()
        .expect("current unix timestamp should fit in u64 nanoseconds")
}
