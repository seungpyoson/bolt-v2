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
    MuEstimatorConfig, MuHealthConfig, MuHealthReason, UsableMu, mint_usable_mu,
};
// `estimate_informed_fraction` / `evaluate_mu_health` are read only by the
// test-only `mu_for` / `health_for` observability helpers below; production reads
// μ exclusively through `mint_usable_mu`, so these imports are `#[cfg(test)]` to
// stay clean of `-D unused-imports` in the production build.
#[cfg(test)]
use crate::bolt_v3_maker_mu_estimator::{estimate_informed_fraction, evaluate_mu_health};
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

    /// The raw informed-fraction μ for `instrument_id` as of `now_ms`, or `None`
    /// if the instrument is unseen or the estimator cannot produce a μ.
    ///
    /// Private and `#[cfg(test)]`: a raw μ is gate-unchecked, so production
    /// consumers must read μ only through the fail-closed
    /// [`usable_mu_for`](Self::usable_mu_for) (which derives μ from
    /// [`estimate_informed_fraction`] directly, not through this helper). This
    /// building block exists solely so unit tests can assert the raw μ independent
    /// of the gate, so it is compiled out of the production binary.
    #[cfg(test)]
    fn mu_for(&self, instrument_id: &InstrumentId, now_ms: u64) -> Option<f64> {
        let flow = self.flows.get(instrument_id)?;
        estimate_informed_fraction(flow, now_ms, &self.estimator)
    }

    /// The fail-closed health verdict for `instrument_id` as of `now_ms`. An
    /// instrument with no buffer yet is [`MuHealthReason::Absent`] (fail-closed);
    /// `None` means healthy.
    ///
    /// The staleness reference is the newest trade **inside the retention window
    /// as of `now_ms`** — exactly the point-in-time view
    /// [`estimate_informed_fraction`] reduces μ over (`samples_within`), not the
    /// raw buffer tail. Anchoring on the raw tail would let a future-dated or
    /// out-of-window trade read as fresh and mask the staleness of the data μ is
    /// actually computed from. Private and `#[cfg(test)]` for the same reason as
    /// [`mu_for`](Self::mu_for): production reads the verdict only inside the gate
    /// (`mint_usable_mu`), so this test-only observability helper is compiled out
    /// of the production binary.
    #[cfg(test)]
    fn health_for(&self, instrument_id: &InstrumentId, now_ms: u64) -> Option<MuHealthReason> {
        let Some(flow) = self.flows.get(instrument_id) else {
            return Some(MuHealthReason::Absent);
        };
        let mu = estimate_informed_fraction(flow, now_ms, &self.estimator);
        let last_trade_ms = flow
            .samples_within(now_ms)
            .last()
            .map(|sample| sample.ts_ms);
        evaluate_mu_health(mu, last_trade_ms, now_ms, &self.health)
    }

    /// The single gate-checked μ read for consumers (quoting, go-live). Returns
    /// `Ok(UsableMu)` only when the health gate passes, and `Err(reason)` carrying
    /// the fail-closed block reason otherwise — so a consumer cannot obtain a μ
    /// without first clearing the gate. The `Ok` value is the [`UsableMu`]
    /// newtype, not a bare `f64`, so the gate-cleared μ is the only μ the quote
    /// planner can be constructed with (the bypass becomes a compile error). This
    /// is the only public μ accessor; the raw `mu_for` / `health_for` building
    /// blocks are private and test-only (`#[cfg(test)]`) so no production quoting
    /// path can bypass the gate.
    pub fn usable_mu_for(
        &self,
        instrument_id: &InstrumentId,
        now_ms: u64,
    ) -> Result<UsableMu, MuHealthReason> {
        // The mint lives with the `UsableMu` type in `bolt_v3_maker_mu_estimator`
        // and runs the health gate internally, so this is the only path to a
        // `UsableMu` and the gate cannot be skipped: `UsableMu::new` is private to
        // that module (a mint anywhere else is a compile error). An unseen
        // instrument has no flow → `Absent`, matching `health_for`'s fail-closed
        // verdict. The staleness anchor is no longer passed in: `mint_usable_mu`
        // derives it internally as the newest sample inside the retention window as
        // of `now_ms` (not the raw tail), so no caller here can forge a fresh
        // reference to slip a stale-but-in-window μ past the gate.
        let Some(flow) = self.flows.get(instrument_id) else {
            return Err(MuHealthReason::Absent);
        };
        mint_usable_mu(flow, now_ms, &self.estimator, &self.health)
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
        // The gate-checked read fails closed for an unseen instrument.
        assert_eq!(
            state.usable_mu_for(&InstrumentId::from(INSTRUMENT_A), NOW_MS),
            Err(MuHealthReason::Absent)
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
        // The only public read returns the gate-cleared μ once the gate passes.
        assert_eq!(
            state
                .usable_mu_for(&InstrumentId::from(INSTRUMENT_A), NOW_MS)
                .map(UsableMu::get),
            Ok(1.0)
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
        // A degenerate μ is gated even though the raw read would hand back 0.0.
        assert_eq!(
            state.usable_mu_for(&InstrumentId::from(INSTRUMENT_A), NOW_MS),
            Err(MuHealthReason::BelowFloor)
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

    // Newest in-window trade 96s before `now`: inside the 600s retention window
    // (so μ is producible) but past the 60s stale window, so the gate blocks Stale.
    const STALE_NOW_MS: u64 = 100_000;

    #[test]
    fn in_window_but_stale_flow_blocks_stale() {
        // Drives the fail-closed Stale branch through `health_for`: four one-sided
        // buys whose newest is 96s old yield a producible μ that the staleness
        // gate must still block.
        let mut state = state();
        let id = InstrumentId::from(INSTRUMENT_A);
        observe_sides(&mut state, INSTRUMENT_A, &[AggressorSide::Buyer; 4]);
        assert_eq!(state.mu_for(&id, STALE_NOW_MS), Some(1.0));
        assert_eq!(
            state.health_for(&id, STALE_NOW_MS),
            Some(MuHealthReason::Stale)
        );
        assert_eq!(
            state.usable_mu_for(&id, STALE_NOW_MS),
            Err(MuHealthReason::Stale)
        );
    }

    #[test]
    fn future_dated_trade_does_not_mask_in_window_staleness() {
        // Pins the staleness anchor to the newest *in-window* trade, not the raw
        // buffer tail. Four stale in-window buys (newest 96s old) plus one
        // future-dated trade: the future tick is the raw tail but is excluded from
        // the in-window view μ uses, so anchoring on the tail would read fresh and
        // pass a stale μ. The gate must report Stale. This assertion fails on the
        // prior `samples().back()` anchor (which returns healthy here) and passes
        // on the `samples_within(now).last()` anchor.
        let mut state = state();
        let id = InstrumentId::from(INSTRUMENT_A);
        observe_sides(&mut state, INSTRUMENT_A, &[AggressorSide::Buyer; 4]);
        // ts 200_000 > STALE_NOW_MS (100_000): newest in the raw buffer, future.
        state.observe(&trade(INSTRUMENT_A, AggressorSide::Buyer, 200_000));
        assert_eq!(
            state.health_for(&id, STALE_NOW_MS),
            Some(MuHealthReason::Stale)
        );
        assert_eq!(
            state.usable_mu_for(&id, STALE_NOW_MS),
            Err(MuHealthReason::Stale)
        );
    }

    #[test]
    fn fresh_newest_in_window_trade_keeps_flow_healthy() {
        // Pins the anchor to the *newest* in-window trade, not the oldest: an old
        // buy (69s before `now`, stale if it were the anchor) plus four fresh buys
        // (newest 2s old) stays healthy. Asserting healthy fails if the anchor were
        // the oldest in-window sample.
        let mut state = state();
        let id = InstrumentId::from(INSTRUMENT_A);
        let now_ms = 70_000;
        state.observe(&trade(INSTRUMENT_A, AggressorSide::Buyer, 1_000));
        for index in 0..4u64 {
            state.observe(&trade(
                INSTRUMENT_A,
                AggressorSide::Buyer,
                65_000 + index * 1_000,
            ));
        }
        assert_eq!(state.health_for(&id, now_ms), None);
        assert_eq!(state.usable_mu_for(&id, now_ms).map(UsableMu::get), Ok(1.0));
    }

    #[test]
    fn usable_mu_is_constructed_only_from_the_gate_cleared_value() {
        // MU-3 seam: the gate-cleared μ surfaces through the `UsableMu` newtype and
        // `get()` returns exactly the μ the estimator produced (here a one-sided
        // buy flow → 1.0). A consumer can only obtain this `UsableMu` from the gate
        // (the field is private and has no public constructor), which is what makes
        // routing a raw, ungated `f64` into the quote planner a compile error.
        let mut state = state();
        observe_sides(&mut state, INSTRUMENT_A, &[AggressorSide::Buyer; 4]);
        let usable = state
            .usable_mu_for(&InstrumentId::from(INSTRUMENT_A), NOW_MS)
            .expect("one-sided warmup flow clears the μ gate");
        assert_eq!(usable.get(), 1.0);
    }
}
