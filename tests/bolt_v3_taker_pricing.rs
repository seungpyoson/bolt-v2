use bolt_v2::bolt_v3_taker_pricing::{
    FastSpotObservation, TakerPricingBlockReason, TakerPricingConfig, TakerPricingRequest,
    TakerPricingState,
};
use bolt_v2::bolt_v3_volatility::RealizedVolConfig;

fn pricing_config() -> TakerPricingConfig {
    TakerPricingConfig {
        realized_vol: RealizedVolConfig {
            window_secs: 60,
            gap_reset_secs: 30,
            min_observations: 3,
            bridge_valid_secs: 10,
        },
        lead_agreement_min_corr: 0.80,
        lead_jitter_max_ms: 1_000,
        spike_guard_return_threshold: 0.50,
        spike_guard_cooldown_secs: 1,
        cadence_seconds: 300,
        theta_decay_factor: 1.5,
        edge_threshold_basis_points: 10,
        pricing_kurtosis: 0.0,
        rotating_market_family: "updown".to_string(),
    }
}

fn observe_pair(
    pricing: &mut TakerPricingState,
    config: &TakerPricingConfig,
    ts_ms: u64,
    price: f64,
) {
    pricing.observe_reference_quote(&FastSpotObservation {
        venue: "reference".to_string(),
        price,
        observed_ts_ms: ts_ms,
    });
    pricing.observe_signal_quote(
        &FastSpotObservation {
            venue: "bybit".to_string(),
            price,
            observed_ts_ms: ts_ms,
        },
        config,
    );
}

fn assert_close(actual: f64, expected: f64) {
    assert!(
        (actual - expected).abs() < 1e-9,
        "expected {expected}, got {actual}"
    );
}

#[test]
fn taker_pricing_returns_current_rv_source_theta_and_fair_probabilities() {
    let config = pricing_config();
    let mut pricing = TakerPricingState::from_config(&config);

    for (ts_ms, price) in [
        (1_000, 3_100.0),
        (2_000, 3_101.0),
        (3_000, 3_102.0),
        (4_000, 3_104.0),
    ] {
        observe_pair(&mut pricing, &config, ts_ms, price);
    }

    let result = pricing
        .entry_pricing_at(
            &config,
            TakerPricingRequest {
                now_ms: 4_000,
                strike_price: Some(3_100.0),
                seconds_to_market_end: Some(300),
            },
        )
        .expect("warmed current taker pricing state should be ready");

    assert!(result.realized_vol > 0.0);
    assert_eq!(result.realized_vol_source_venue.as_deref(), Some("bybit"));
    assert_eq!(result.realized_vol_source_ts_ms, Some(4_000));
    assert_close(result.theta_scaled_min_edge_bps, 10.0);
    assert!(result.fair_probability_up > 0.5);
    assert_close(
        result.fair_probability_down,
        1.0 - result.fair_probability_up,
    );
}

#[test]
fn taker_pricing_reports_current_readiness_blockers_without_strategy_order_state() {
    let config = pricing_config();
    let pricing = TakerPricingState::from_config(&config);

    let blocked = pricing
        .entry_pricing_at(
            &config,
            TakerPricingRequest {
                now_ms: 1_000,
                strike_price: None,
                seconds_to_market_end: None,
            },
        )
        .expect_err("cold pricing state should fail closed with current blockers");

    assert_eq!(
        blocked,
        vec![
            TakerPricingBlockReason::SpotPriceMissing,
            TakerPricingBlockReason::StrikePriceMissing,
            TakerPricingBlockReason::SecondsToExpiryMissing,
            TakerPricingBlockReason::RealizedVolNotReady,
            TakerPricingBlockReason::ThetaScalerUnavailable,
        ]
    );
}

#[test]
fn taker_pricing_accepts_source_owned_realized_vol_seed_without_strategy_estimator_access() {
    let config = pricing_config();
    let mut pricing = TakerPricingState::from_config(&config);

    pricing.seed_ready_realized_vol(Some("reference".to_string()), 2.5, 1_000);

    assert_eq!(pricing.current_realized_vol_at(1_000), Some(2.5));
    assert_eq!(
        pricing.current_realized_vol_source_at(1_000),
        (Some("reference".to_string()), Some(1_000))
    );

    pricing.seed_ready_realized_vol(Some("older".to_string()), 3.0, 999);

    assert_eq!(pricing.current_realized_vol_at(1_000), Some(2.5));
    assert_eq!(
        pricing.current_realized_vol_source_at(1_000),
        (Some("reference".to_string()), Some(1_000))
    );
}
