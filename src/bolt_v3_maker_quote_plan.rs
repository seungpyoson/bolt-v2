//! Shared pure maker quote planner.
//!
//! This module is the bridge between the existing maker primitives and the
//! market-family quote seam. It owns no strategy state and performs no NT calls:
//! callers supply the current fair value, toxicity, inventory, top-of-book, and
//! configured quote-layout knobs, then receive family-specific quote targets.

use crate::{
    bolt_v3_maker_microprice::{micro_price, micro_price_anchor},
    bolt_v3_maker_model::{gm_binary_quote, inventory_skew},
    bolt_v3_maker_mu_estimator::UsableMu,
    bolt_v3_market_families::maker_quote_targets_for_family,
    bolt_v3_quoting::{FamilyQuoteInputs, QuoteTargets},
};

/// Already-extracted top-of-book inputs for the outcome being anchored.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MakerTopOfBook {
    pub best_bid: f64,
    pub best_ask: f64,
    pub bid_size: f64,
    pub ask_size: f64,
}

/// Config- and state-sourced inputs for one maker quote planning tick.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MakerQuotePlanInputs<'a> {
    pub family_key: &'a str,
    pub oracle_fair_probability_up: f64,
    /// The informed-fraction μ, carried as the gate-cleared [`UsableMu`] newtype
    /// rather than a bare `f64`. This makes "μ reached `gm_binary_quote` without
    /// clearing the health gate" a compile error: the only way to produce a
    /// `UsableMu` is the fail-closed μ gate, and its raw value is read with
    /// `.get()` only at the `gm_binary_quote` call below.
    pub informed_fraction: UsableMu,
    pub top_of_book: Option<MakerTopOfBook>,
    pub microprice_weight: f64,
    pub net_position: f64,
    pub inventory_skew_gain: f64,
    pub position_cap: f64,
    pub half_spread_floor: f64,
    pub max_half_spread: f64,
    pub eps: f64,
    pub tau: f64,
    pub reference_tau: f64,
    pub time_widen_cap: f64,
    pub order_notional_target: f64,
    pub maximum_position_notional: f64,
}

/// Planned family-specific maker quote targets plus the intermediate economic
/// values that explain the plan.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MakerQuotePlan {
    pub fair_probability_up: f64,
    pub reservation_bid: f64,
    pub reservation_ask: f64,
    pub inventory_skew: f64,
    pub targets: QuoteTargets,
}

pub fn plan_maker_quote_targets(inputs: MakerQuotePlanInputs<'_>) -> Option<MakerQuotePlan> {
    let micro = inputs
        .top_of_book
        .and_then(|book| micro_price(book.best_bid, book.best_ask, book.bid_size, book.ask_size));
    let fair_probability_up = micro_price_anchor(
        inputs.oracle_fair_probability_up,
        micro,
        inputs.microprice_weight,
    )?;
    let reservation = gm_binary_quote(fair_probability_up, inputs.informed_fraction.get())?;
    let inventory_skew = inventory_skew(
        inputs.net_position,
        inputs.inventory_skew_gain,
        inputs.position_cap,
    )?;
    let targets = maker_quote_targets_for_family(
        inputs.family_key,
        FamilyQuoteInputs {
            band: reservation,
            inventory_skew,
            half_spread_floor: inputs.half_spread_floor,
            max_half_spread: inputs.max_half_spread,
            eps: inputs.eps,
            tau: inputs.tau,
            reference_tau: inputs.reference_tau,
            time_widen_cap: inputs.time_widen_cap,
            order_notional_target: inputs.order_notional_target,
            maximum_position_notional: inputs.maximum_position_notional,
        },
    )?;

    Some(MakerQuotePlan {
        fair_probability_up,
        reservation_bid: reservation.bid(),
        reservation_ask: reservation.ask(),
        inventory_skew,
        targets,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bolt_v3_maker_model::gm_binary_quote;
    use crate::bolt_v3_market_families::updown;

    const FAIR_UP: f64 = 0.60;
    const TEST_MU: f64 = 0.04;
    const REF_TAU: f64 = 3_600.0;
    const WIDEN_CAP: f64 = 10.0;
    const TEST_EPS: f64 = 1e-6;
    const POSITION_CAP: f64 = 100.0;
    const ORDER_NOTIONAL_TARGET: f64 = 50.0;
    const MAXIMUM_POSITION_NOTIONAL: f64 = 500.0;
    const EPSILON: f64 = 1e-12;

    /// Inputs that anchor a non-degenerate interior plan: no microprice (so the
    /// oracle fair passes through), neutral inventory, and the reference horizon
    /// (widening factor 1) with a zero floor — the same known-good shape the
    /// `bolt_v3_quoting` layout tests use. `informed_fraction` is the gate-cleared
    /// [`UsableMu`] newtype, minted here through its in-crate constructor exactly as
    /// the μ gate does.
    fn plan_inputs(informed_fraction: UsableMu) -> MakerQuotePlanInputs<'static> {
        MakerQuotePlanInputs {
            family_key: updown::KEY,
            oracle_fair_probability_up: FAIR_UP,
            informed_fraction,
            top_of_book: None,
            microprice_weight: 0.0,
            net_position: 0.0,
            inventory_skew_gain: 0.0,
            position_cap: POSITION_CAP,
            half_spread_floor: 0.0,
            max_half_spread: 1.0,
            eps: TEST_EPS,
            tau: REF_TAU,
            reference_tau: REF_TAU,
            time_widen_cap: WIDEN_CAP,
            order_notional_target: ORDER_NOTIONAL_TARGET,
            maximum_position_notional: MAXIMUM_POSITION_NOTIONAL,
        }
    }

    #[test]
    fn gate_cleared_mu_flows_unchanged_into_gm_binary_quote() {
        // MU-3 positive path: the μ value reaching `gm_binary_quote` is EXACTLY the
        // one carried by the `UsableMu` the gate produced. The planner reads it via
        // `informed_fraction.get()` at the single model call, so the resulting
        // reservation band must equal `gm_binary_quote(fair, mu)` computed directly
        // with the same μ. (No top-of-book → the oracle fair is the band's fair, so
        // the comparison is exact.) A seam that dropped or defaulted the gated μ —
        // e.g. read 0.0 instead of `.get()` — would collapse the GM band to
        // bid==ask==fair and fail the strict-inequality assertions below.
        let mu = UsableMu::for_test(TEST_MU);
        let plan = plan_maker_quote_targets(plan_inputs(mu)).expect("interior inputs plan");

        let expected = gm_binary_quote(FAIR_UP, TEST_MU).expect("interior band");
        assert!((plan.reservation_bid - expected.bid()).abs() < EPSILON);
        assert!((plan.reservation_ask - expected.ask()).abs() < EPSILON);
        assert!((plan.fair_probability_up - FAIR_UP).abs() < EPSILON);
        // The carried μ is non-degenerate, so the band is a genuine two-sided
        // spread around the fair — proving the value was not silently zeroed.
        assert!(plan.reservation_bid < FAIR_UP);
        assert!(plan.reservation_ask > FAIR_UP);
    }

    #[test]
    fn distinct_gate_cleared_mu_values_produce_distinct_bands() {
        // A second positive-path pin: a higher gate-cleared μ (more toxic flow)
        // must widen the reservation band. If the seam ignored `informed_fraction`
        // and used a constant, both plans would be identical and this fails. This is
        // the differential half of the value-flow guard at the planner boundary.
        let narrow = plan_maker_quote_targets(plan_inputs(UsableMu::for_test(TEST_MU)))
            .expect("low-mu interior plan");
        let wide = plan_maker_quote_targets(plan_inputs(UsableMu::for_test(TEST_MU * 4.0)))
            .expect("high-mu interior plan");

        assert!(
            wide.reservation_ask - wide.reservation_bid
                > narrow.reservation_ask - narrow.reservation_bid,
            "a higher gate-cleared μ must widen the reservation band"
        );
    }
}
