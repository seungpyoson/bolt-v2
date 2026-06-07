use std::collections::BTreeMap;

use bolt_v2::bolt_v3_realized_volatility::{
    RealizedVolAggregation, RealizedVolBlockReason, RealizedVolSnapshot,
};
use bolt_v2::bolt_v3_taker_pricing::{
    FastSpotObservation, TakerPricingBlockReason, TakerPricingConfig, TakerPricingRequest,
    TakerPricingState,
};
use bolt_v2::bolt_v3_volatility::RealizedVolConfig;

fn pricing_config_with_family(rotating_market_family: &'static str) -> TakerPricingConfig<'static> {
    TakerPricingConfig {
        realized_vol: Some(RealizedVolConfig {
            window_secs: 60,
            gap_reset_secs: 30,
            min_observations: 3,
            bridge_valid_secs: 10,
        }),
        lead_agreement_min_corr: 0.80,
        lead_jitter_max_ms: 1_000,
        spike_guard_return_threshold: 0.50,
        spike_guard_cooldown_secs: 1,
        cadence_seconds: 300,
        theta_decay_factor: 1.5,
        edge_threshold_basis_points: 10,
        pricing_kurtosis: 0.0,
        rotating_market_family,
        realized_volatility_surface_id: None,
    }
}

fn pricing_config() -> TakerPricingConfig<'static> {
    pricing_config_with_family("updown")
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

fn warm_legacy_internal_realized_vol_estimator(
    pricing: &mut TakerPricingState,
    config: &TakerPricingConfig<'_>,
) {
    for (ts_ms, price) in [
        (1_000, 3_100.0),
        (2_000, 3_101.0),
        (3_000, 3_102.0),
        (4_000, 3_104.0),
    ] {
        observe_pair(pricing, config, ts_ms, price);
    }
}

#[test]
fn taker_pricing_consumes_realized_vol_snapshot_without_internal_estimator_warmup() {
    let mut config = pricing_config();
    config.realized_volatility_surface_id = Some("<surface_id>".to_string());
    let mut pricing = TakerPricingState::from_config(&config);
    pricing.observe_reference_quote(&FastSpotObservation {
        venue: "<REFERENCE_SOURCE_ID>".to_string(),
        price: 3_100.0,
        observed_ts_ms: 1_000,
    });
    pricing.observe_signal_quote(
        &FastSpotObservation {
            venue: "<SIGNAL_SOURCE_ID>".to_string(),
            price: 3_100.0,
            observed_ts_ms: 1_000,
        },
        &config,
    );
    pricing.observe_realized_vol_snapshot(RealizedVolSnapshot {
        surface_id: "<surface_id>".to_string(),
        as_of_ms: 1_000,
        annualized_realized_vol_decimal: Some(2.5),
        ready: true,
        sources_used: vec!["<SOURCE_ID_A>".to_string()],
        source_diagnostics: Vec::new(),
        unknown_source_rejections: BTreeMap::new(),
        blocked_reasons: Vec::new(),
        aggregate_method: RealizedVolAggregation::UpperQuantile { quantile: 1.0 },
        seconds_per_annum: 31_536_000.0,
        config_fingerprint: "<config_fingerprint>".to_string(),
    });

    let result = pricing
        .entry_pricing_at(
            &config,
            TakerPricingRequest {
                now_ms: 1_000,
                strike_price: Some(3_100.0),
                seconds_to_market_end: Some(300),
            },
        )
        .expect("ready realized-volatility snapshot should satisfy taker pricing");

    assert_close(result.realized_vol, 2.5);
    assert_eq!(
        result.realized_vol_surface_id.as_deref(),
        Some("<surface_id>")
    );
    assert_eq!(result.realized_vol_source_venue, None);
    assert_eq!(result.realized_vol_source_ts_ms, Some(1_000));
}

#[test]
fn taker_pricing_accepts_ready_surfaced_zero_realized_volatility_snapshot() {
    let mut config = pricing_config();
    config.realized_volatility_surface_id = Some("<surface_id>".to_string());
    let mut pricing = TakerPricingState::from_config(&config);
    pricing.observe_reference_quote(&FastSpotObservation {
        venue: "<REFERENCE_SOURCE_ID>".to_string(),
        price: 3_101.0,
        observed_ts_ms: 1_000,
    });
    pricing.observe_signal_quote(
        &FastSpotObservation {
            venue: "<SIGNAL_SOURCE_ID>".to_string(),
            price: 3_101.0,
            observed_ts_ms: 1_000,
        },
        &config,
    );
    pricing.observe_realized_vol_snapshot(RealizedVolSnapshot {
        surface_id: "<surface_id>".to_string(),
        as_of_ms: 1_000,
        annualized_realized_vol_decimal: Some(0.0),
        ready: true,
        sources_used: vec!["<SOURCE_ID_A>".to_string()],
        source_diagnostics: Vec::new(),
        unknown_source_rejections: BTreeMap::new(),
        blocked_reasons: Vec::new(),
        aggregate_method: RealizedVolAggregation::UpperQuantile { quantile: 1.0 },
        seconds_per_annum: 31_536_000.0,
        config_fingerprint: "<config_fingerprint>".to_string(),
    });

    let result = pricing
        .entry_pricing_at(
            &config,
            TakerPricingRequest {
                now_ms: 1_000,
                strike_price: Some(3_100.0),
                seconds_to_market_end: Some(300),
            },
        )
        .expect("ready zero-RV surfaced snapshot should satisfy taker pricing");

    assert_close(result.realized_vol, 0.0);
    assert_close(result.fair_probability_up, 1.0);
    assert_eq!(
        result.realized_vol_surface_id.as_deref(),
        Some("<surface_id>")
    );
    assert_eq!(result.realized_vol_source_ts_ms, Some(1_000));
}

#[test]
fn surfaced_realized_volatility_mode_blocks_instead_of_falling_back_to_legacy_estimator() {
    let mut config = pricing_config();
    config.realized_volatility_surface_id = Some("<surface_id>".to_string());
    let mut pricing = TakerPricingState::from_config(&config);
    warm_legacy_internal_realized_vol_estimator(&mut pricing, &config);
    pricing.observe_realized_vol_snapshot(RealizedVolSnapshot {
        surface_id: "<surface_id>".to_string(),
        as_of_ms: 1_000,
        annualized_realized_vol_decimal: None,
        ready: false,
        sources_used: Vec::new(),
        source_diagnostics: Vec::new(),
        unknown_source_rejections: BTreeMap::new(),
        blocked_reasons: vec![RealizedVolBlockReason::QuorumNotReady],
        aggregate_method: RealizedVolAggregation::UpperQuantile { quantile: 1.0 },
        seconds_per_annum: 31_536_000.0,
        config_fingerprint: "<config_fingerprint>".to_string(),
    });

    let err = pricing
        .entry_pricing_at(
            &config,
            TakerPricingRequest {
                now_ms: 1_000,
                strike_price: Some(3_100.0),
                seconds_to_market_end: Some(300),
            },
        )
        .expect_err("not-ready surfaced realized-volatility snapshot must fail closed");

    assert!(err.contains(&TakerPricingBlockReason::RealizedVolNotReady));
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

    assert_close(result.realized_vol, 2.5608173247578083);
    assert_eq!(result.realized_vol_source_venue.as_deref(), Some("bybit"));
    assert_eq!(result.realized_vol_source_ts_ms, Some(4_000));
    assert_close(result.theta_scaled_min_edge_bps, 10.0);
    assert_close(result.fair_probability_up, 0.5633110689151639);
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
fn taker_pricing_reports_fair_probability_unavailable_after_inputs_are_ready() {
    let config = pricing_config_with_family("fixture_unregistered_family");
    let mut pricing = TakerPricingState::from_config(&config);

    for (ts_ms, price) in [
        (1_000, 3_100.0),
        (2_000, 3_101.0),
        (3_000, 3_102.0),
        (4_000, 3_104.0),
    ] {
        observe_pair(&mut pricing, &config, ts_ms, price);
    }

    let blocked = pricing
        .entry_pricing_at(
            &config,
            TakerPricingRequest {
                now_ms: 4_000,
                strike_price: Some(3_100.0),
                seconds_to_market_end: Some(300),
            },
        )
        .expect_err("unknown market family should fail after pricing inputs are ready");

    assert_eq!(
        blocked,
        vec![TakerPricingBlockReason::FairProbabilityUnavailable]
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

    pricing.seed_ready_realized_vol(None, 3.0, 1_001);

    assert_eq!(pricing.current_realized_vol_at(1_001), Some(3.0));
    assert_eq!(
        pricing.current_realized_vol_source_at(1_001),
        (None, Some(1_001))
    );

    pricing.seed_ready_realized_vol(Some("zero".to_string()), 0.0, 1_002);

    assert_eq!(pricing.current_realized_vol_at(1_002), Some(0.0));
    assert_eq!(
        pricing.current_realized_vol_source_at(1_002),
        (Some("zero".to_string()), Some(1_002))
    );
}

#[test]
fn taker_pricing_rejects_invalid_source_owned_realized_vol_seed() {
    let config = pricing_config();
    let mut pricing = TakerPricingState::from_config(&config);

    pricing.seed_ready_realized_vol(Some("reference".to_string()), 2.5, 1_000);
    pricing.seed_ready_realized_vol(Some("negative".to_string()), -0.01, 1_001);
    pricing.seed_ready_realized_vol(Some("nan".to_string()), f64::NAN, 1_002);

    assert_eq!(pricing.current_realized_vol_at(1_002), Some(2.5));
    assert_eq!(
        pricing.current_realized_vol_source_at(1_002),
        (Some("reference".to_string()), Some(1_000))
    );
}

#[test]
fn source_owned_seed_without_venue_falls_back_to_current_fast_spot_source() {
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

    assert_eq!(
        pricing.current_realized_vol_source_at(4_000),
        (Some("bybit".to_string()), Some(4_000))
    );

    pricing.seed_ready_realized_vol(Some("reference".to_string()), 8.0, 4_000);

    assert_eq!(
        pricing.current_realized_vol_source_at(4_000),
        (Some("reference".to_string()), Some(4_000))
    );

    pricing.seed_ready_realized_vol(None, 9.0, 4_001);

    assert_eq!(pricing.current_realized_vol_at(4_001), Some(9.0));
    assert_eq!(
        pricing.current_realized_vol_source_at(4_001),
        (Some("bybit".to_string()), Some(4_001))
    );
}
