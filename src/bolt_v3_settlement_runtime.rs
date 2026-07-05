use std::{path::PathBuf, rc::Rc};

use anyhow::Result;

use crate::{
    bolt_v3_loss_protection::PositionRealizedPnlObservation,
    bolt_v3_venue_truth::VenueTruthSettlementExplanation,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoltV3SettlementRecoveryConfig {
    pub path: PathBuf,
    pub max_bytes: u64,
}

pub trait BoltV3SettlementRuntimeSink: std::fmt::Debug {
    fn record_loss_governor_position_realized_pnl(
        &self,
        observation: PositionRealizedPnlObservation,
    ) -> Result<()>;

    fn record_venue_truth_settlement(
        &self,
        explanation: VenueTruthSettlementExplanation,
    ) -> Result<()>;
}

pub type BoltV3SettlementRuntimeSinkHandle = Rc<dyn BoltV3SettlementRuntimeSink>;
