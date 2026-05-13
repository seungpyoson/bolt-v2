use crate::bolt_v3_live_canary_gate::BoltV3LiveCanaryGateReport;
use rust_decimal::Decimal;
use std::sync::Mutex;

#[derive(Debug)]
pub struct BoltV3SubmitAdmissionState {
    inner: Mutex<BoltV3SubmitAdmissionInner>,
}

#[derive(Debug)]
struct BoltV3SubmitAdmissionInner {
    gate_report: Option<BoltV3LiveCanaryGateReport>,
    admitted_order_count: u32,
}

impl BoltV3SubmitAdmissionState {
    pub fn new_unarmed() -> Self {
        Self {
            inner: Mutex::new(BoltV3SubmitAdmissionInner {
                gate_report: None,
                admitted_order_count: 0,
            }),
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
        let report = inner
            .gate_report
            .as_ref()
            .ok_or(BoltV3SubmitAdmissionError::NotArmed)?;
        if request.notional <= Decimal::ZERO {
            return Err(BoltV3SubmitAdmissionError::NonPositiveNotional);
        }
        if request.notional > report.max_notional_per_order() {
            return Err(BoltV3SubmitAdmissionError::NotionalCapExceeded);
        }
        if inner.admitted_order_count >= report.max_live_order_count() {
            return Err(BoltV3SubmitAdmissionError::CountCapExhausted);
        }

        inner.admitted_order_count += 1;
        Ok(BoltV3SubmitAdmissionPermit(()))
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
        }
    }
}

impl std::error::Error for BoltV3SubmitAdmissionError {}
