use bolt_v2::{
    bolt_v3_maker_quote_plan::{MakerQuotePlanInputs, MakerTopOfBook, plan_maker_quote_targets},
    bolt_v3_market_families::static_binary_event_family_key,
    bolt_v3_quoting::QuoteSide,
};

const EPSILON: f64 = 1e-9;

fn quote_inputs() -> MakerQuotePlanInputs<'static> {
    MakerQuotePlanInputs {
        family_key: static_binary_event_family_key(),
        oracle_fair_probability_up: 0.60,
        informed_fraction: 0.20,
        top_of_book: Some(MakerTopOfBook {
            best_bid: 0.58,
            best_ask: 0.62,
            bid_size: 4_000.0,
            ask_size: 1_000.0,
        }),
        microprice_weight: 0.50,
        net_position: 3.0,
        inventory_skew_gain: 0.001,
        position_cap: 100.0,
        half_spread_floor: 0.01,
        max_half_spread: 0.30,
        eps: 0.001,
        tau: 3_600.0,
        reference_tau: 3_600.0,
        time_widen_cap: 2.0,
    }
}

#[test]
fn static_binary_event_quote_plan_uses_existing_binary_family_layout() {
    let plan = plan_maker_quote_targets(quote_inputs()).expect("quote plan should be valid");

    assert_eq!(plan.targets.leg_a.side, QuoteSide::Buy);
    assert_eq!(plan.targets.leg_b.side, QuoteSide::Buy);
    assert!(plan.targets.leg_a.price > 0.0);
    assert!(plan.targets.leg_b.price > 0.0);
    assert!(plan.targets.leg_a.price + plan.targets.leg_b.price < 1.0);
    assert!(plan.reservation_bid < plan.fair_probability_up);
    assert!(plan.fair_probability_up < plan.reservation_ask);
    assert!((plan.inventory_skew - 0.003).abs() < EPSILON);
}

#[test]
fn maker_quote_plan_fails_closed_at_inventory_cap() {
    let mut inputs = quote_inputs();
    inputs.net_position = inputs.position_cap;

    assert!(plan_maker_quote_targets(inputs).is_none());
}

#[test]
fn maker_quote_plan_falls_back_to_oracle_fair_when_book_is_degenerate() {
    let mut inputs = quote_inputs();
    inputs.top_of_book = Some(MakerTopOfBook {
        best_bid: 0.62,
        best_ask: 0.58,
        bid_size: 4_000.0,
        ask_size: 1_000.0,
    });

    let plan = plan_maker_quote_targets(inputs).expect("degenerate book should not block quote");

    assert!((plan.fair_probability_up - inputs.oracle_fair_probability_up).abs() < EPSILON);
}
