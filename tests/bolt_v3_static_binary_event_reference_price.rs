use bolt_v2::bolt_v3_market_families::{
    FairProbabilityInputs, fair_probability_up_for_family, static_binary_event_family_key,
};

#[test]
fn static_binary_event_uses_reference_current_price_probability() {
    let fair_probability = fair_probability_up_for_family(
        static_binary_event_family_key(),
        &FairProbabilityInputs {
            spot_price: 0.63,
            strike_price: f64::NAN,
            seconds_to_market_end: 0,
            realized_vol: f64::NAN,
            pricing_kurtosis: f64::NAN,
        },
    );

    assert_eq!(fair_probability, Some(0.63));
}
