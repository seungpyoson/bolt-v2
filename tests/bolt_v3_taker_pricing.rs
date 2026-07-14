use std::collections::BTreeMap;

use bolt_v2::bolt_v3_realized_volatility::{
    RealizedVolAggregation, RealizedVolBlockReason, RealizedVolPricingComponent,
    RealizedVolSnapshot,
};
use bolt_v2::bolt_v3_taker_pricing::{
    FastSpotObservation, TakerPricingBlockReason, TakerPricingConfig, TakerPricingRequest,
    TakerPricingState,
};
use bolt_v2::bolt_v3_timestamp_domain::{LocalReceiveMs, VenueEventMs};

fn pricing_config_with_family(rotating_market_family: &'static str) -> TakerPricingConfig<'static> {
    TakerPricingConfig {
        realized_volatility_surface_id: "<surface_id>".to_string(),
        realized_volatility_max_source_age_ms: None,
        lead_agreement_min_corr: 0.80,
        lead_jitter_max_ms: 1_000,
        spike_guard_return_threshold: 0.50,
        spike_guard_cooldown_secs: 1,
        cadence_seconds: 300,
        theta_decay_factor: 1.5,
        edge_threshold_basis_points: 10,
        pricing_kurtosis: 0.0,
        rotating_market_family,
        max_reference_current_price_age_ms: Some(2_000),
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
    pricing.observe_reference_current_price(&FastSpotObservation {
        venue: "reference".to_string(),
        price,
        observed_ts_ms: ts_ms,
        received_ts_ms: None,
    });
    pricing.observe_signal_quote(
        &FastSpotObservation {
            venue: "bybit".to_string(),
            price,
            observed_ts_ms: ts_ms,
            received_ts_ms: None,
        },
        config,
    );
}

fn seed_ready_realized_vol(
    pricing: &mut TakerPricingState,
    source_id: Option<String>,
    realized_vol: f64,
    ready_ts_ms: u64,
) {
    if !realized_vol.is_finite() || realized_vol < 0.0 {
        return;
    }
    pricing.observe_realized_vol_snapshot(RealizedVolSnapshot {
        surface_id: "<surface_id>".to_string(),
        as_of_ms: ready_ts_ms,
        latest_accepted_receive_ms: Some(LocalReceiveMs::new(ready_ts_ms)),
        annualized_realized_vol_decimal: Some(realized_vol),
        measured_annualized_realized_vol_decimal: Some(realized_vol),
        noise_robust_annualized_realized_vol_decimal: Some(realized_vol),
        continuous_annualized_realized_vol_decimal: Some(realized_vol),
        jump_annualized_realized_vol_decimal: Some(0.0),
        forecast_annualized_realized_vol_decimal: None,
        pricing_component: RealizedVolPricingComponent::Measured,
        ready: true,
        sources_used: source_id.into_iter().collect(),
        source_diagnostics: Vec::new(),
        horizon_estimates: Vec::new(),
        unknown_source_rejections: BTreeMap::new(),
        blocked_reasons: Vec::new(),
        aggregate_method: RealizedVolAggregation::UpperQuantile { quantile: 1.0 },
        seconds_per_annum: 31_536_000.0,
        config_fingerprint: String::new(),
    });
}

fn assert_close(actual: f64, expected: f64) {
    assert!(
        (actual - expected).abs() < 1e-9,
        "expected {expected}, got {actual}"
    );
}

#[test]
fn taker_pricing_consumes_realized_vol_snapshot_without_internal_estimator_warmup() {
    let config = pricing_config();
    let mut pricing = TakerPricingState::from_config(&config);
    pricing.observe_reference_current_price(&FastSpotObservation {
        venue: "<REFERENCE_SOURCE_ID>".to_string(),
        price: 3_100.0,
        observed_ts_ms: 1_000,
        received_ts_ms: None,
    });
    pricing.observe_signal_quote(
        &FastSpotObservation {
            venue: "<SIGNAL_SOURCE_ID>".to_string(),
            price: 3_100.0,
            observed_ts_ms: 1_000,
            received_ts_ms: None,
        },
        &config,
    );
    pricing.observe_realized_vol_snapshot(RealizedVolSnapshot {
        surface_id: "<surface_id>".to_string(),
        as_of_ms: 1_000,
        latest_accepted_receive_ms: Some(LocalReceiveMs::new(1_000)),
        annualized_realized_vol_decimal: Some(2.5),
        measured_annualized_realized_vol_decimal: Some(2.5),
        noise_robust_annualized_realized_vol_decimal: Some(2.5),
        continuous_annualized_realized_vol_decimal: Some(2.5),
        jump_annualized_realized_vol_decimal: Some(0.0),
        forecast_annualized_realized_vol_decimal: None,
        pricing_component: RealizedVolPricingComponent::Measured,
        ready: true,
        sources_used: vec!["<SOURCE_ID_A>".to_string()],
        source_diagnostics: Vec::new(),
        horizon_estimates: Vec::new(),
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
                realized_vol_gate_receive_ms: LocalReceiveMs::new(1_000),
                reference_gate_event_ms: Some(VenueEventMs::new(1_000)),
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
    assert_eq!(
        result.realized_vol_source_venue.as_deref(),
        Some("<SOURCE_ID_A>")
    );
    assert_eq!(result.realized_vol_source_ts_ms, Some(1_000));
}

#[test]
fn taker_entry_pricing_does_not_compare_independent_venue_clocks() {
    let config = pricing_config();
    let mut pricing = TakerPricingState::from_config(&config);
    observe_pair(&mut pricing, &config, 1_000, 3_100.0);
    let mut snapshot = realized_vol_snapshot_for_surface("<surface_id>", 2.5, 1_001);
    snapshot.latest_accepted_receive_ms = Some(LocalReceiveMs::new(999));
    snapshot.sources_used = vec!["<SOURCE_ID_A>".to_string()];
    pricing.observe_realized_vol_snapshot(snapshot);

    let result = pricing.entry_pricing_at(
        &config,
        TakerPricingRequest {
            now_ms: 1_000,
            realized_vol_gate_receive_ms: LocalReceiveMs::new(1_000),
            reference_gate_event_ms: Some(VenueEventMs::new(1_000)),
            strike_price: Some(3_100.0),
            seconds_to_market_end: Some(300),
        },
    );

    assert!(
        result.is_ok(),
        "entry pricing must not reject an RV source venue clock that leads by one millisecond: {result:?}"
    );
}

#[test]
fn taker_pricing_accepts_ready_surfaced_zero_realized_volatility_snapshot() {
    let config = pricing_config();
    let mut pricing = TakerPricingState::from_config(&config);
    pricing.observe_reference_current_price(&FastSpotObservation {
        venue: "<REFERENCE_SOURCE_ID>".to_string(),
        price: 3_101.0,
        observed_ts_ms: 1_000,
        received_ts_ms: None,
    });
    pricing.observe_signal_quote(
        &FastSpotObservation {
            venue: "<SIGNAL_SOURCE_ID>".to_string(),
            price: 3_101.0,
            observed_ts_ms: 1_000,
            received_ts_ms: None,
        },
        &config,
    );
    pricing.observe_realized_vol_snapshot(RealizedVolSnapshot {
        surface_id: "<surface_id>".to_string(),
        as_of_ms: 1_000,
        latest_accepted_receive_ms: Some(LocalReceiveMs::new(1_000)),
        annualized_realized_vol_decimal: Some(0.0),
        measured_annualized_realized_vol_decimal: Some(0.0),
        noise_robust_annualized_realized_vol_decimal: Some(0.0),
        continuous_annualized_realized_vol_decimal: Some(0.0),
        jump_annualized_realized_vol_decimal: Some(0.0),
        forecast_annualized_realized_vol_decimal: None,
        pricing_component: RealizedVolPricingComponent::Measured,
        ready: true,
        sources_used: vec!["<SOURCE_ID_A>".to_string()],
        source_diagnostics: Vec::new(),
        horizon_estimates: Vec::new(),
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
                realized_vol_gate_receive_ms: LocalReceiveMs::new(1_000),
                reference_gate_event_ms: Some(VenueEventMs::new(1_000)),
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
    let config = pricing_config();
    let mut pricing = TakerPricingState::from_config(&config);
    observe_pair(&mut pricing, &config, 1_000, 3_100.0);
    pricing.observe_realized_vol_snapshot(RealizedVolSnapshot {
        surface_id: "<surface_id>".to_string(),
        as_of_ms: 1_000,
        latest_accepted_receive_ms: Some(LocalReceiveMs::new(1_000)),
        annualized_realized_vol_decimal: None,
        measured_annualized_realized_vol_decimal: None,
        noise_robust_annualized_realized_vol_decimal: None,
        continuous_annualized_realized_vol_decimal: None,
        jump_annualized_realized_vol_decimal: None,
        forecast_annualized_realized_vol_decimal: None,
        pricing_component: RealizedVolPricingComponent::Measured,
        ready: false,
        sources_used: Vec::new(),
        source_diagnostics: Vec::new(),
        horizon_estimates: Vec::new(),
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
                realized_vol_gate_receive_ms: LocalReceiveMs::new(1_000),
                reference_gate_event_ms: Some(VenueEventMs::new(1_000)),
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

    observe_pair(&mut pricing, &config, 4_000, 3_104.0);
    seed_ready_realized_vol(&mut pricing, Some("<SOURCE_ID_A>".to_string()), 2.5, 4_000);

    let result = pricing
        .entry_pricing_at(
            &config,
            TakerPricingRequest {
                now_ms: 4_000,
                realized_vol_gate_receive_ms: LocalReceiveMs::new(4_000),
                reference_gate_event_ms: Some(VenueEventMs::new(4_000)),
                strike_price: Some(3_100.0),
                seconds_to_market_end: Some(300),
            },
        )
        .expect("warmed current taker pricing state should be ready");

    assert_close(result.realized_vol, 2.5);
    assert_eq!(
        result.realized_vol_surface_id.as_deref(),
        Some("<surface_id>")
    );
    assert_eq!(
        result.realized_vol_source_venue.as_deref(),
        Some("<SOURCE_ID_A>")
    );
    assert_eq!(result.realized_vol_source_ts_ms, Some(4_000));
    assert_close(result.theta_scaled_min_edge_bps, 10.0);
    assert!(result.fair_probability_up.is_finite());
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
                realized_vol_gate_receive_ms: LocalReceiveMs::new(1_000),
                reference_gate_event_ms: Some(VenueEventMs::new(1_000)),
                strike_price: None,
                seconds_to_market_end: None,
            },
        )
        .expect_err("cold pricing state should fail closed with current blockers");

    assert_eq!(
        blocked,
        vec![
            TakerPricingBlockReason::SpotPriceMissing,
            TakerPricingBlockReason::ReferenceCurrentPriceStale,
            TakerPricingBlockReason::StrikePriceMissing,
            TakerPricingBlockReason::SecondsToExpiryMissing,
            TakerPricingBlockReason::RealizedVolNotReady,
            TakerPricingBlockReason::ThetaScalerUnavailable,
        ]
    );
}

#[test]
fn taker_pricing_reports_stale_signal_spot_with_other_fair_value_blockers() {
    let config = pricing_config();
    let mut pricing = TakerPricingState::from_config(&config);

    observe_pair(&mut pricing, &config, 1_000, 3_100.0);
    pricing.observe_reference_current_price(&FastSpotObservation {
        venue: "reference".to_string(),
        price: 3_101.0,
        observed_ts_ms: 3_000,
        received_ts_ms: None,
    });

    let blocked = pricing
        .entry_pricing_inputs_at(
            &config,
            TakerPricingRequest {
                now_ms: 3_001,
                realized_vol_gate_receive_ms: LocalReceiveMs::new(3_001),
                reference_gate_event_ms: Some(VenueEventMs::new(3_001)),
                strike_price: None,
                seconds_to_market_end: Some(300),
            },
        )
        .expect_err("stale signal spot must remain visible with shared FV blockers");

    assert_eq!(
        blocked,
        vec![
            TakerPricingBlockReason::SpotPriceMissing,
            TakerPricingBlockReason::StrikePriceMissing,
            TakerPricingBlockReason::RealizedVolNotReady,
        ]
    );
}

#[test]
fn shared_fair_value_pricing_stays_available_when_taker_theta_is_unavailable() {
    let mut config = pricing_config();
    config.cadence_seconds = 0;
    let mut pricing = TakerPricingState::from_config(&config);
    observe_pair(&mut pricing, &config, 1_000, 100.0);
    seed_ready_realized_vol(&mut pricing, Some("bybit".to_string()), 0.50, 1_000);
    let request = TakerPricingRequest {
        now_ms: 1_000,
        realized_vol_gate_receive_ms: LocalReceiveMs::new(1_000),
        reference_gate_event_ms: Some(VenueEventMs::new(1_000)),
        strike_price: Some(100.0),
        seconds_to_market_end: Some(300),
    };

    let fair_value = pricing
        .fair_value_pricing_at(&config, request)
        .expect("shared fair-value inputs should not depend on taker theta");

    assert_eq!(fair_value.spot_price, 100.0);
    assert_eq!(fair_value.strike_price, 100.0);
    assert_eq!(fair_value.seconds_to_market_end, 300);
    assert_eq!(fair_value.realized_vol, 0.50);
    assert!(fair_value.fair_probability_up.is_finite());
    assert_eq!(
        pricing.entry_pricing_inputs_at(&config, request),
        Err(vec![TakerPricingBlockReason::ThetaScalerUnavailable])
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
    seed_ready_realized_vol(&mut pricing, Some("<SOURCE_ID_A>".to_string()), 2.5, 4_000);

    let blocked = pricing
        .entry_pricing_at(
            &config,
            TakerPricingRequest {
                now_ms: 4_000,
                realized_vol_gate_receive_ms: LocalReceiveMs::new(4_000),
                reference_gate_event_ms: Some(VenueEventMs::new(4_000)),
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

    seed_ready_realized_vol(&mut pricing, Some("reference".to_string()), 2.5, 1_000);

    assert_eq!(
        pricing.current_realized_vol_at(LocalReceiveMs::new(1_000), None),
        Some(2.5)
    );
    assert_eq!(
        pricing.current_realized_vol_source_at(LocalReceiveMs::new(1_000), None),
        (Some("reference".to_string()), Some(1_000))
    );

    seed_ready_realized_vol(&mut pricing, Some("older".to_string()), 3.0, 999);

    assert_eq!(
        pricing.current_realized_vol_at(LocalReceiveMs::new(1_000), None),
        Some(2.5)
    );
    assert_eq!(
        pricing.current_realized_vol_source_at(LocalReceiveMs::new(1_000), None),
        (Some("reference".to_string()), Some(1_000))
    );

    seed_ready_realized_vol(&mut pricing, None, 3.0, 1_001);

    assert_eq!(
        pricing.current_realized_vol_at(LocalReceiveMs::new(1_001), None),
        Some(3.0)
    );
    assert_eq!(
        pricing.current_realized_vol_source_at(LocalReceiveMs::new(1_001), None),
        (None, Some(1_001))
    );

    seed_ready_realized_vol(&mut pricing, Some("zero".to_string()), 0.0, 1_002);

    assert_eq!(
        pricing.current_realized_vol_at(LocalReceiveMs::new(1_002), None),
        Some(0.0)
    );
    assert_eq!(
        pricing.current_realized_vol_source_at(LocalReceiveMs::new(1_002), None),
        (Some("zero".to_string()), Some(1_002))
    );
}

#[test]
fn taker_pricing_rejects_invalid_source_owned_realized_vol_seed() {
    let config = pricing_config();
    let mut pricing = TakerPricingState::from_config(&config);

    seed_ready_realized_vol(&mut pricing, Some("reference".to_string()), 2.5, 1_000);
    seed_ready_realized_vol(&mut pricing, Some("negative".to_string()), -0.01, 1_001);
    seed_ready_realized_vol(&mut pricing, Some("nan".to_string()), f64::NAN, 1_002);

    assert_eq!(
        pricing.current_realized_vol_at(LocalReceiveMs::new(1_002), None),
        Some(2.5)
    );
    assert_eq!(
        pricing.current_realized_vol_source_at(LocalReceiveMs::new(1_002), None),
        (Some("reference".to_string()), Some(1_000))
    );
}

fn realized_vol_snapshot_for_surface(
    surface_id: &str,
    realized_vol: f64,
    ready_ts_ms: u64,
) -> RealizedVolSnapshot {
    RealizedVolSnapshot {
        surface_id: surface_id.to_string(),
        as_of_ms: ready_ts_ms,
        latest_accepted_receive_ms: Some(LocalReceiveMs::new(ready_ts_ms)),
        annualized_realized_vol_decimal: Some(realized_vol),
        measured_annualized_realized_vol_decimal: Some(realized_vol),
        noise_robust_annualized_realized_vol_decimal: Some(realized_vol),
        continuous_annualized_realized_vol_decimal: Some(realized_vol),
        jump_annualized_realized_vol_decimal: Some(0.0),
        forecast_annualized_realized_vol_decimal: None,
        pricing_component: RealizedVolPricingComponent::Measured,
        ready: true,
        sources_used: Vec::new(),
        source_diagnostics: Vec::new(),
        horizon_estimates: Vec::new(),
        unknown_source_rejections: BTreeMap::new(),
        blocked_reasons: Vec::new(),
        aggregate_method: RealizedVolAggregation::UpperQuantile { quantile: 1.0 },
        seconds_per_annum: 31_536_000.0,
        config_fingerprint: String::new(),
    }
}

/// Guard against a single unkeyed RV snapshot slot: a newer-timestamp snapshot from a
/// *foreign* surface must NOT evict the configured surface's snapshot. The shared runtime
/// routes ticks by instrument, and instrument overlap can publish foreign surfaces, so
/// per-surface keying is required (subscription scoping alone cannot guarantee isolation).
#[test]
fn foreign_surface_snapshot_does_not_clobber_configured_surface_readiness() {
    let config = pricing_config(); // configured surface "<surface_id>"
    let mut pricing = TakerPricingState::from_config(&config);

    // Configured surface publishes a ready snapshot.
    pricing.observe_realized_vol_snapshot(realized_vol_snapshot_for_surface(
        "<surface_id>",
        1.5,
        100,
    ));
    // A foreign surface publishes a NEWER snapshot (would have clobbered a single unkeyed
    // slot).
    pricing.observe_realized_vol_snapshot(realized_vol_snapshot_for_surface(
        "<foreign_surface>",
        9.9,
        200,
    ));

    // Configured surface readiness is intact; the foreign snapshot never displaced it.
    // (A `pub(crate)` field probe isn't visible from this external integration test, so the
    // behavior is asserted via the public read API — which is the actual contract.)
    assert_eq!(
        pricing.current_realized_vol_at(LocalReceiveMs::new(201), None),
        Some(1.5)
    );
    assert_eq!(
        pricing.current_realized_vol_source_at(LocalReceiveMs::new(201), None),
        (None, Some(100))
    );
}

/// Per-key monotonic guard: an equal-`as_of_ms` snapshot for the SAME surface replaces
/// (last-writer-wins, matching the prior `<=` semantics); an older one is rejected.
#[test]
fn equal_timestamp_snapshot_replaces_and_older_snapshot_is_rejected_per_surface() {
    let config = pricing_config();
    let mut pricing = TakerPricingState::from_config(&config);

    pricing.observe_realized_vol_snapshot(realized_vol_snapshot_for_surface(
        "<surface_id>",
        1.5,
        100,
    ));
    // Equal timestamp: replaces (refresh vs event at the same `as_of_ms`).
    pricing.observe_realized_vol_snapshot(realized_vol_snapshot_for_surface(
        "<surface_id>",
        2.0,
        100,
    ));
    assert_eq!(
        pricing.current_realized_vol_at(LocalReceiveMs::new(100), None),
        Some(2.0)
    );

    // Older timestamp for the same surface: rejected, newer value preserved.
    pricing.observe_realized_vol_snapshot(realized_vol_snapshot_for_surface(
        "<surface_id>",
        9.9,
        50,
    ));
    assert_eq!(
        pricing.current_realized_vol_at(LocalReceiveMs::new(100), None),
        Some(2.0)
    );
}

/// Multi-instance readiness non-interference: two pricing states configured for
/// different surfaces, both fed by the same shared runtime (so both observe every published
/// snapshot). Each must read only its own configured surface; one surface's snapshot must not
/// affect the other instance's readiness, and a newer foreign snapshot must not affect the
/// configured surface's value.
#[test]
fn two_pricing_instances_with_distinct_configured_surfaces_do_not_interfere() {
    let mut config_a = pricing_config();
    config_a.realized_volatility_surface_id = "<surface_a>".to_string();
    let mut config_b = pricing_config();
    config_b.realized_volatility_surface_id = "<surface_b>".to_string();
    let mut pricing_a = TakerPricingState::from_config(&config_a);
    let mut pricing_b = TakerPricingState::from_config(&config_b);

    // Shared runtime publishes surface A's snapshot to BOTH instances.
    let snap_a = realized_vol_snapshot_for_surface("<surface_a>", 1.5, 100);
    pricing_a.observe_realized_vol_snapshot(snap_a.clone());
    pricing_b.observe_realized_vol_snapshot(snap_a);

    // Instance A reads its configured surface; instance B (configured for surface_b) is not
    // ready yet, and surface A's snapshot does NOT make B ready.
    assert_eq!(
        pricing_a.current_realized_vol_at(LocalReceiveMs::new(101), None),
        Some(1.5)
    );
    assert_eq!(
        pricing_b.current_realized_vol_at(LocalReceiveMs::new(101), None),
        None
    );

    // Shared runtime publishes surface B's snapshot (newer ts) to BOTH. Must not evict A.
    let snap_b = realized_vol_snapshot_for_surface("<surface_b>", 2.5, 200);
    pricing_a.observe_realized_vol_snapshot(snap_b.clone());
    pricing_b.observe_realized_vol_snapshot(snap_b);

    // Each instance reads only its own configured surface; no cross-surface interference.
    assert_eq!(
        pricing_a.current_realized_vol_at(LocalReceiveMs::new(201), None),
        Some(1.5)
    );
    assert_eq!(
        pricing_b.current_realized_vol_at(LocalReceiveMs::new(201), None),
        Some(2.5)
    );
}
