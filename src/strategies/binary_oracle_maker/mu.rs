//! Per-instrument μ runtime state for the binary-oracle maker (Slice 2, #488).
//!
//! Owns one shared signed trade-flow buffer per quoted instrument, observes
//! trades into it, and derives the informed-fraction μ plus the fail-closed
//! health verdict from the shared [`crate::bolt_v3_maker_mu_estimator`]. This is
//! the per-strategy state the estimator's pure functions read; no orders are
//! emitted — quoting arrives in Slice 6.

use std::collections::BTreeMap;

use nautilus_model::{data::TradeTick, identifiers::InstrumentId};

use crate::bolt_v3_maker_mu_estimator::{
    MuEstimatorConfig, MuHealthConfig, MuHealthReason, estimate_informed_fraction,
    evaluate_mu_health,
};
use crate::bolt_v3_trade_flow::{SignedTradeFlow, SignedTradeFlowConfig};

/// Per-instrument signed trade-flow buffers plus the projected μ-estimator and
/// health-gate config views. Built from the maker's parsed TOML knobs; one
/// `SignedTradeFlow` is created lazily per quoted instrument.
#[derive(Debug, Clone)]
pub struct MakerMuState {
    estimator: MuEstimatorConfig,
    health: MuHealthConfig,
    flow_config: SignedTradeFlowConfig,
    flows: BTreeMap<InstrumentId, SignedTradeFlow>,
}

impl MakerMuState {
    /// Build empty per-instrument state from the projected config views.
    pub fn new(
        estimator: MuEstimatorConfig,
        health: MuHealthConfig,
        flow_config: SignedTradeFlowConfig,
    ) -> Self {
        Self {
            estimator,
            health,
            flow_config,
            flows: BTreeMap::new(),
        }
    }

    /// Route a trade into its instrument's buffer, creating the buffer on first
    /// sight from the projected [`SignedTradeFlowConfig`].
    pub fn observe(&mut self, trade: &TradeTick) {
        self.flows
            .entry(trade.instrument_id)
            .or_insert_with(|| SignedTradeFlow::from_config(&self.flow_config))
            .observe(trade);
    }

    /// The informed-fraction μ for `instrument_id` as of `now_ms`, or `None` if
    /// the instrument is unseen or the estimator cannot produce a μ.
    pub fn mu_for(&self, instrument_id: &InstrumentId, now_ms: u64) -> Option<f64> {
        let flow = self.flows.get(instrument_id)?;
        estimate_informed_fraction(flow, now_ms, &self.estimator)
    }

    /// The fail-closed health verdict for `instrument_id` as of `now_ms`. An
    /// instrument with no buffer yet is [`MuHealthReason::Absent`] (fail-closed);
    /// `None` means healthy. The staleness reference is the most recent retained
    /// trade timestamp.
    pub fn health_for(&self, instrument_id: &InstrumentId, now_ms: u64) -> Option<MuHealthReason> {
        let Some(flow) = self.flows.get(instrument_id) else {
            return Some(MuHealthReason::Absent);
        };
        let mu = estimate_informed_fraction(flow, now_ms, &self.estimator);
        let last_trade_ms = flow.samples().back().map(|sample| sample.ts_ms);
        evaluate_mu_health(mu, last_trade_ms, now_ms, &self.health)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bolt_v3_numeric::NANOS_PER_MILLI_U64;
    use nautilus_core::UnixNanos;
    use nautilus_model::{
        enums::AggressorSide,
        identifiers::TradeId,
        types::{Price, Quantity},
    };

    const PRICE_PRECISION: u8 = 2;
    const SIZE_PRECISION: u8 = 0;
    const TRADE_PRICE: f64 = 0.5;
    const UNIT_SIZE: f64 = 1.0;
    const WINDOW_SECS: u64 = 600;
    const MAX_SAMPLES: u64 = 1_000;
    const MIN_CLASSIFIED: u64 = 4;
    const STALE_WINDOW_MS: u64 = 60_000;
    const MU_MIN_FLOOR: f64 = 0.05;
    const FIRST_TS_MS: u64 = 1_000;
    const STEP_MS: u64 = 1_000;
    const NOW_MS: u64 = 50_000;
    const INSTRUMENT_A: &str = "MUA.SIM";
    const INSTRUMENT_B: &str = "MUB.SIM";

    fn state() -> MakerMuState {
        MakerMuState::new(
            MuEstimatorConfig {
                min_classified_samples: MIN_CLASSIFIED,
            },
            MuHealthConfig {
                stale_window_ms: STALE_WINDOW_MS,
                mu_min_floor: MU_MIN_FLOOR,
            },
            SignedTradeFlowConfig {
                window_secs: WINDOW_SECS,
                max_samples: MAX_SAMPLES,
            },
        )
    }

    fn trade(instrument: &str, aggressor: AggressorSide, ts_ms: u64) -> TradeTick {
        let ts_ns = ts_ms * NANOS_PER_MILLI_U64;
        TradeTick::new_checked(
            InstrumentId::from(instrument),
            Price::new(TRADE_PRICE, PRICE_PRECISION),
            Quantity::new(UNIT_SIZE, SIZE_PRECISION),
            aggressor,
            TradeId::from(format!("T{ts_ns}").as_str()),
            UnixNanos::from(ts_ns),
            UnixNanos::from(ts_ns),
        )
        .expect("valid test trade tick")
    }

    fn observe_sides(state: &mut MakerMuState, instrument: &str, sides: &[AggressorSide]) {
        for (index, side) in sides.iter().enumerate() {
            state.observe(&trade(
                instrument,
                *side,
                FIRST_TS_MS + (index as u64) * STEP_MS,
            ));
        }
    }

    #[test]
    fn unknown_instrument_is_absent() {
        let state = state();
        assert_eq!(
            state.health_for(&InstrumentId::from(INSTRUMENT_A), NOW_MS),
            Some(MuHealthReason::Absent)
        );
        assert_eq!(
            state.mu_for(&InstrumentId::from(INSTRUMENT_A), NOW_MS),
            None
        );
    }

    #[test]
    fn one_sided_flow_is_healthy() {
        let mut state = state();
        observe_sides(&mut state, INSTRUMENT_A, &[AggressorSide::Buyer; 4]);
        assert_eq!(
            state.mu_for(&InstrumentId::from(INSTRUMENT_A), NOW_MS),
            Some(1.0)
        );
        assert_eq!(
            state.health_for(&InstrumentId::from(INSTRUMENT_A), NOW_MS),
            None
        );
    }

    #[test]
    fn balanced_flow_blocks_below_floor() {
        let mut state = state();
        observe_sides(
            &mut state,
            INSTRUMENT_A,
            &[
                AggressorSide::Buyer,
                AggressorSide::Seller,
                AggressorSide::Buyer,
                AggressorSide::Seller,
            ],
        );
        assert_eq!(
            state.mu_for(&InstrumentId::from(INSTRUMENT_A), NOW_MS),
            Some(0.0)
        );
        assert_eq!(
            state.health_for(&InstrumentId::from(INSTRUMENT_A), NOW_MS),
            Some(MuHealthReason::BelowFloor)
        );
    }

    #[test]
    fn instruments_are_tracked_independently() {
        let mut state = state();
        observe_sides(&mut state, INSTRUMENT_A, &[AggressorSide::Buyer; 4]);
        assert_eq!(
            state.health_for(&InstrumentId::from(INSTRUMENT_B), NOW_MS),
            Some(MuHealthReason::Absent)
        );
        assert_eq!(
            state.health_for(&InstrumentId::from(INSTRUMENT_A), NOW_MS),
            None
        );
    }
}
