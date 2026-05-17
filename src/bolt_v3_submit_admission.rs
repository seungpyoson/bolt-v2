use crate::bolt_v3_decision_evidence::{
    BoltV3AdmissionDecisionEvidence, BoltV3AdmissionOutcome, BoltV3DecisionEvidenceWriter,
};
use crate::bolt_v3_live_canary_gate::BoltV3LiveCanaryGateReport;
use rust_decimal::Decimal;
use std::sync::{Arc, Mutex};

#[derive(Debug)]
pub struct BoltV3SubmitAdmissionState {
    inner: Mutex<BoltV3SubmitAdmissionInner>,
    decision_evidence: Arc<dyn BoltV3DecisionEvidenceWriter>,
}

#[derive(Debug)]
struct BoltV3SubmitAdmissionInner {
    gate_report: Option<BoltV3LiveCanaryGateReport>,
    admitted_order_count: u32,
}

impl BoltV3SubmitAdmissionState {
    pub fn new_unarmed(decision_evidence: Arc<dyn BoltV3DecisionEvidenceWriter>) -> Self {
        Self {
            inner: Mutex::new(BoltV3SubmitAdmissionInner {
                gate_report: None,
                admitted_order_count: 0,
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
        Ok(())
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
            client_order_id: request.client_order_id.clone(),
            instrument_id: request.instrument_id.clone(),
            notional: request.notional.to_string(),
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
                Ok(BoltV3SubmitAdmissionPermit(()))
            }
            BoltV3AdmissionOutcome::RejectedNotArmed => Err(BoltV3SubmitAdmissionError::NotArmed),
            BoltV3AdmissionOutcome::RejectedNonPositiveNotional => {
                Err(BoltV3SubmitAdmissionError::NonPositiveNotional)
            }
            BoltV3AdmissionOutcome::RejectedNotionalCapExceeded => {
                Err(BoltV3SubmitAdmissionError::NotionalCapExceeded)
            }
            BoltV3AdmissionOutcome::RejectedCountCapExhausted => {
                Err(BoltV3SubmitAdmissionError::CountCapExhausted)
            }
        }
    }

    fn evaluate(
        inner: &BoltV3SubmitAdmissionInner,
        request: &BoltV3SubmitAdmissionRequest,
    ) -> BoltV3AdmissionOutcome {
        let Some(report) = inner.gate_report.as_ref() else {
            return BoltV3AdmissionOutcome::RejectedNotArmed;
        };
        if request.notional <= Decimal::ZERO {
            return BoltV3AdmissionOutcome::RejectedNonPositiveNotional;
        }
        if request.notional > report.max_notional_per_order() {
            return BoltV3AdmissionOutcome::RejectedNotionalCapExceeded;
        }
        if inner.admitted_order_count >= report.max_live_order_count() {
            return BoltV3AdmissionOutcome::RejectedCountCapExhausted;
        }
        BoltV3AdmissionOutcome::Admitted
    }

    pub fn admitted_order_count(&self) -> u32 {
        self.inner
            .lock()
            .expect("submit admission state mutex should not be poisoned")
            .admitted_order_count
    }
}

#[derive(Debug)]
pub struct BoltV3SubmitAdmissionPermit(());

#[derive(Debug)]
pub struct BoltV3SubmitAdmissionRequest {
    pub strategy_id: String,
    pub client_order_id: String,
    pub instrument_id: String,
    pub notional: Decimal,
}

#[derive(Debug, Eq, PartialEq)]
pub enum BoltV3SubmitAdmissionError {
    NotArmed,
    AlreadyArmed,
    CountCapExhausted,
    NonPositiveNotional,
    NotionalCapExceeded,
    EvidenceWriteFailed { reason: String },
}

impl std::fmt::Display for BoltV3SubmitAdmissionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotArmed => write!(f, "bolt-v3 submit admission is not armed"),
            Self::AlreadyArmed => write!(f, "bolt-v3 submit admission is already armed"),
            Self::CountCapExhausted => {
                write!(f, "bolt-v3 submit admission order count cap is exhausted")
            }
            Self::NonPositiveNotional => {
                write!(f, "bolt-v3 submit admission notional must be positive")
            }
            Self::NotionalCapExceeded => {
                write!(f, "bolt-v3 submit admission notional cap is exceeded")
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
