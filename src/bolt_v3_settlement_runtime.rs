use std::{path::PathBuf, rc::Rc};

use anyhow::Result;

use crate::{
    bolt_v3_config::BoltV3RootConfig, bolt_v3_loss_protection::PositionRealizedPnlObservation,
    bolt_v3_venue_truth::VenueTruthSettlementExplanation,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoltV3SettlementRecoveryConfig {
    pub path: PathBuf,
    pub max_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BoltV3SettlementRuntimeSinkBackends {
    loss_protection: bool,
    capital_admission_runtime_feed: bool,
}

impl BoltV3SettlementRuntimeSinkBackends {
    pub(crate) fn from_root(root: &BoltV3RootConfig) -> Self {
        let loss_protection = root
            .risk
            .kill_switch
            .as_ref()
            .is_some_and(|kill_switch| kill_switch.enabled);
        let capital_admission_runtime_feed =
            root.risk.capital_pools.as_ref().is_some_and(|pools| {
                pools.iter().any(|pool| {
                    pool.enforce_submit_admission && pool.prediction_market_binary.is_some()
                })
            });
        Self {
            loss_protection,
            capital_admission_runtime_feed,
        }
    }

    pub(crate) fn will_configure_runtime_sink(self) -> bool {
        self.loss_protection || self.capital_admission_runtime_feed
    }

    pub(crate) fn loss_protection(self) -> bool {
        self.loss_protection
    }

    pub(crate) fn capital_admission_runtime_feed(self) -> bool {
        self.capital_admission_runtime_feed
    }
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
