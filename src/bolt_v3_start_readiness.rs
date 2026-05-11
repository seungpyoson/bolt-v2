use nautilus_live::node::LiveNode;

use crate::{
    bolt_v3_config::LoadedBoltV3Config,
    bolt_v3_instrument_readiness::{
        BoltV3InstrumentReadinessReport, check_bolt_v3_instrument_readiness_for_start,
    },
    bolt_v3_market_families::updown::BoltV3MarketIdentityError,
};

/// Pre-start readiness surface for bolt-v3.
///
/// This gate is intentionally narrower than production approval. It
/// composes accepted pre-start checks that can be evaluated while the NT
/// `LiveNode` is still idle. It must not call NT `start`, `run`, order APIs,
/// or user subscription APIs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BoltV3StartReadinessReport {
    pub instrument_readiness: BoltV3InstrumentReadinessReport,
}

impl BoltV3StartReadinessReport {
    pub fn is_ready(&self) -> bool {
        self.instrument_readiness.is_ready()
    }
}

#[derive(Debug)]
pub enum BoltV3StartReadinessGateError {
    MarketIdentity(BoltV3MarketIdentityError),
    Blocked(BoltV3StartReadinessReport),
}

impl std::fmt::Display for BoltV3StartReadinessGateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BoltV3StartReadinessGateError::MarketIdentity(error) => write!(f, "{error}"),
            BoltV3StartReadinessGateError::Blocked(report) => write!(
                f,
                "bolt-v3 start readiness gate blocked production start with {} instrument readiness fact(s)",
                report.instrument_readiness.facts.len()
            ),
        }
    }
}

impl std::error::Error for BoltV3StartReadinessGateError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            BoltV3StartReadinessGateError::MarketIdentity(error) => Some(error),
            BoltV3StartReadinessGateError::Blocked(_) => None,
        }
    }
}

impl From<BoltV3MarketIdentityError> for BoltV3StartReadinessGateError {
    fn from(error: BoltV3MarketIdentityError) -> Self {
        BoltV3StartReadinessGateError::MarketIdentity(error)
    }
}

pub fn check_bolt_v3_start_readiness_gate(
    node: &LiveNode,
    loaded: &LoadedBoltV3Config,
    market_selection_timestamp_milliseconds: i64,
) -> Result<BoltV3StartReadinessReport, BoltV3MarketIdentityError> {
    let instrument_readiness = check_bolt_v3_instrument_readiness_for_start(
        node,
        loaded,
        market_selection_timestamp_milliseconds,
    )?;
    Ok(BoltV3StartReadinessReport {
        instrument_readiness,
    })
}

pub fn require_bolt_v3_start_readiness_gate(
    node: &LiveNode,
    loaded: &LoadedBoltV3Config,
    market_selection_timestamp_milliseconds: i64,
) -> Result<BoltV3StartReadinessReport, BoltV3StartReadinessGateError> {
    let report =
        check_bolt_v3_start_readiness_gate(node, loaded, market_selection_timestamp_milliseconds)?;
    if report.is_ready() {
        Ok(report)
    } else {
        Err(BoltV3StartReadinessGateError::Blocked(report))
    }
}
