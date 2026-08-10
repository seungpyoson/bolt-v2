//! Shared maker primitives for binary-outcome market families.

use crate::{
    bolt_v3_binary_settlement::BinarySettlementPayout,
    bolt_v3_numeric::ZERO_F64,
    bolt_v3_quote_lifecycle::Leg,
    bolt_v3_quoting::{
        FamilyQuoteInputs, QuoteSide, QuoteTargetLeg, QuoteTargets, compose_binary_legs,
    },
    bolt_v3_sizing::maker_robust_size,
};

pub fn maker_quote_targets(inputs: FamilyQuoteInputs) -> Option<QuoteTargets> {
    // The band carries an already-sanitized fair (`gm_binary_quote` is the sole
    // producer and sanitizes `p_up` at mint), so the layout consumes it directly.
    let legs = compose_binary_legs(
        inputs.band,
        inputs.half_spread_floor,
        inputs.max_half_spread,
        inputs.tau,
        inputs.reference_tau,
        inputs.time_widen_cap,
        inputs.inventory_skew,
        inputs.eps,
    )?;
    // Size the legs off the protective half-spread the maker captures, NOT off
    // directional EV (the GM/CG maker is break-even, so the taker EV-gated sizer
    // would force perpetual zero-size quotes). A non-positive edge sizes to zero,
    // which is a fail-closed no-quote: a priced leg with zero notional is not a
    // real maker quote.
    let size_notional = maker_robust_size(
        inputs.band.half_spread(),
        inputs.max_half_spread,
        inputs.order_notional_target,
        inputs.maximum_position_notional,
    );
    if size_notional <= ZERO_F64 {
        return None;
    }
    Some(QuoteTargets {
        leg_a: QuoteTargetLeg {
            side: QuoteSide::Buy,
            price: legs.yes_price,
            size_notional,
        },
        leg_b: QuoteTargetLeg {
            side: QuoteSide::Buy,
            price: legs.no_price,
            size_notional,
        },
    })
}

pub fn settlement_payout(payout: BinarySettlementPayout, leg: Leg) -> Option<f64> {
    Some(payout.leg_payout(leg))
}
