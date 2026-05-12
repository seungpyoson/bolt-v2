use std::sync::Mutex;

use rust_decimal::Decimal;

use crate::bolt_v3_live_canary_gate::BoltV3LiveCanaryGateReport;

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct BoltV3SubmitAdmissionRequest {
    pub strategy_id: String,
    pub client_order_id: String,
    pub instrument_id: String,
    pub notional: Decimal,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct BoltV3SubmitAdmissionPermit {
    pub admitted_order_count: u32,
    pub max_live_order_count: u32,
    pub max_notional_per_order: Decimal,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum BoltV3SubmitAdmissionError {
    MissingGateReport,
    AlreadyArmed,
    InvalidNotional {
        notional: Decimal,
    },
    NotionalExceedsCap {
        notional: Decimal,
        max_notional_per_order: Decimal,
    },
    OrderCountExhausted {
        admitted_order_count: u32,
        max_live_order_count: u32,
    },
    LockPoisoned,
}

impl std::fmt::Display for BoltV3SubmitAdmissionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingGateReport => {
                write!(
                    f,
                    "bolt-v3 submit admission is missing live canary gate report"
                )
            }
            Self::AlreadyArmed => write!(f, "bolt-v3 submit admission is already armed"),
            Self::InvalidNotional { notional } => write!(
                f,
                "bolt-v3 submit admission notional must be positive, got {notional}"
            ),
            Self::NotionalExceedsCap {
                notional,
                max_notional_per_order,
            } => write!(
                f,
                "bolt-v3 submit admission notional {notional} exceeds cap {max_notional_per_order}"
            ),
            Self::OrderCountExhausted {
                admitted_order_count,
                max_live_order_count,
            } => write!(
                f,
                "bolt-v3 submit admission order count exhausted: admitted {admitted_order_count}, max {max_live_order_count}"
            ),
            Self::LockPoisoned => write!(f, "bolt-v3 submit admission lock is poisoned"),
        }
    }
}

impl std::error::Error for BoltV3SubmitAdmissionError {}

#[derive(Debug, Default)]
struct BoltV3SubmitAdmissionInner {
    gate_report: Option<BoltV3LiveCanaryGateReport>,
    admitted_order_count: u32,
}

#[derive(Debug, Default)]
pub struct BoltV3SubmitAdmissionState {
    inner: Mutex<BoltV3SubmitAdmissionInner>,
}

impl BoltV3SubmitAdmissionState {
    pub fn new_unarmed() -> Self {
        Self::default()
    }

    pub fn arm(
        &self,
        report: BoltV3LiveCanaryGateReport,
    ) -> Result<(), BoltV3SubmitAdmissionError> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| BoltV3SubmitAdmissionError::LockPoisoned)?;
        if inner.gate_report.is_some() {
            return Err(BoltV3SubmitAdmissionError::AlreadyArmed);
        }
        inner.gate_report = Some(report);
        Ok(())
    }

    pub fn admit(
        &self,
        request: &BoltV3SubmitAdmissionRequest,
    ) -> Result<BoltV3SubmitAdmissionPermit, BoltV3SubmitAdmissionError> {
        if request.notional <= Decimal::ZERO {
            return Err(BoltV3SubmitAdmissionError::InvalidNotional {
                notional: request.notional,
            });
        }

        let mut inner = self
            .inner
            .lock()
            .map_err(|_| BoltV3SubmitAdmissionError::LockPoisoned)?;
        let Some(report) = inner.gate_report.clone() else {
            return Err(BoltV3SubmitAdmissionError::MissingGateReport);
        };
        if request.notional > report.max_notional_per_order {
            return Err(BoltV3SubmitAdmissionError::NotionalExceedsCap {
                notional: request.notional,
                max_notional_per_order: report.max_notional_per_order,
            });
        }
        if inner.admitted_order_count >= report.max_live_order_count {
            return Err(BoltV3SubmitAdmissionError::OrderCountExhausted {
                admitted_order_count: inner.admitted_order_count,
                max_live_order_count: report.max_live_order_count,
            });
        }

        inner.admitted_order_count += 1;
        Ok(BoltV3SubmitAdmissionPermit {
            admitted_order_count: inner.admitted_order_count,
            max_live_order_count: report.max_live_order_count,
            max_notional_per_order: report.max_notional_per_order,
        })
    }

    pub fn admitted_order_count(&self) -> u32 {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .admitted_order_count
    }
}
