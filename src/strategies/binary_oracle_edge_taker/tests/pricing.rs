#![cfg(test)]

use super::*;

const TEST_PRICING_SNAPSHOT_REFERENCE_STEP_MS: u64 = 100;
const TEST_PRICING_SNAPSHOT_REFERENCE_PRICE_STEP: f64 = 2.0;
const TEST_PRICING_SNAPSHOT_FAST_WEIGHT: f64 = 0.9;
const TEST_PRICING_SNAPSHOT_NEWER_REFERENCE_TS_MS: u64 = 1_100;
const TEST_PRICING_SNAPSHOT_STALE_REFERENCE_TS_MS: u64 =
    TEST_PRICING_SNAPSHOT_NEWER_REFERENCE_TS_MS - TEST_PRICING_SNAPSHOT_REFERENCE_STEP_MS;
const TEST_PRICING_SNAPSHOT_FRESH_SIGNAL_TS_MS: u64 =
    TEST_PRICING_SNAPSHOT_NEWER_REFERENCE_TS_MS + TEST_PRICING_SNAPSHOT_REFERENCE_STEP_MS;
const TEST_PRICING_SNAPSHOT_NEWER_REFERENCE_PRICE: f64 = 3_101.0;
const TEST_PRICING_SNAPSHOT_STALE_REFERENCE_PRICE: f64 =
    TEST_PRICING_SNAPSHOT_NEWER_REFERENCE_PRICE - TEST_PRICING_SNAPSHOT_REFERENCE_PRICE_STEP;
const TEST_PRICING_SNAPSHOT_FRESH_SIGNAL_PRICE: f64 =
    TEST_PRICING_SNAPSHOT_NEWER_REFERENCE_PRICE;
const TEST_PRICING_SNAPSHOT_MISMATCHED_STALE_REFERENCE_PRICE: f64 =
    TEST_PRICING_SNAPSHOT_REFERENCE_PRICE_STEP;

#[test]
fn reference_quote_tick_updates_fair_value_without_becoming_signal() {
    let mut strategy = test_strategy();

    strategy
        .on_quote(&quote_tick("REFERENCE.SOURCE", 100.0, 102.0, 1_200))
        .expect("reference quote should process");

    assert_eq!(strategy.pricing.last_reference_fair_value, Some(101.0));
    assert_eq!(strategy.pricing.fast_spot, None);
    assert!(!strategy.pricing.lead_quality_policy_applied);
}

#[test]
fn signal_quote_tick_updates_pricing_from_configured_signal_data() {
    let mut strategy = test_strategy();

    strategy
        .on_quote(&quote_tick("REFERENCE.SOURCE", 100.0, 102.0, 1_100))
        .expect("reference quote should process");
    strategy
        .on_quote(&quote_tick("SIGNAL.SOURCE", 100.5, 102.5, 1_200))
        .expect("signal quote should process");

    assert_eq!(strategy.pricing.last_reference_fair_value, Some(101.0));
    assert_eq!(
        strategy.pricing.fast_spot,
        Some(fast_spot("signal_data_client", 101.5, 1_200))
    );
    assert!(strategy.pricing.lead_quality_policy_applied);
}

#[test]
fn signal_quote_tick_does_not_warm_active_reference_state() {
    let mut strategy = test_strategy();
    let mut market = candidate_market("market-1", 1_000);
    market.price_to_beat = Some(3_100.0);
    strategy.apply_selection_snapshot(selection_snapshot(1_000, SelectionState::Active { market }));
    strategy.pricing.last_reference_fair_value = Some(3_101.0);

    strategy
        .on_quote(&quote_tick("SIGNAL.SOURCE", 3_102.0, 3_104.0, 1_200))
        .expect("signal quote should process");

    assert_eq!(strategy.active.interval_open, None);
    assert_eq!(strategy.active.last_reference_ts_ms, None);
    assert_eq!(strategy.active.warmup_count, INITIAL_COUNTER_U64);
    assert_eq!(
        strategy.pricing.fast_spot,
        Some(fast_spot("signal_data_client", 3_103.0, 1_200))
    );
}

#[test]
fn non_reference_quote_tick_does_not_update_pricing() {
    let mut strategy = test_strategy();

    strategy
        .on_quote(&quote_tick("OTHER.SOURCE", 100.0, 102.0, 1_200))
        .expect("non-reference quote should be ignored");

    assert_eq!(strategy.pricing.last_reference_fair_value, None);
    assert_eq!(strategy.pricing.fast_spot, None);
}

#[test]
fn pricing_state_requires_fast_spot_for_pricing_and_keeps_reference_separate() {
    let config = test_strategy().config.clone();
    let mut pricing = PricingState::from_config(&taker_pricing_config(&config));

    pricing.observe_reference_snapshot(
        &reference_tick(1_000, 3_100.0),
        config.lead_agreement_min_corr,
        config.lead_jitter_max_ms,
    );
    assert_eq!(pricing.spot_price(), None);
    assert_eq!(pricing.last_reference_fair_value, Some(3_100.0));

    let snapshot = ReferenceSnapshot {
        ts_ms: 1_100,
        topic: "platform.reference.test.spot".to_string(),
        fair_value: Some(3_101.0),
        confidence: 1.0,
        venues: vec![
            oracle_venue("reference", 1.0, 3_101.0, 1_100),
            orderbook_venue("bybit", 0.9, 3_102.0, 1_100),
        ],
    };
    pricing.observe_reference_snapshot(
        &snapshot,
        config.lead_agreement_min_corr,
        config.lead_jitter_max_ms,
    );
    assert_eq!(pricing.spot_price(), Some(3_102.0));
}

#[test]
fn pricing_state_reference_snapshot_rejects_stale_fair_value() {
    let config = test_strategy().config.clone();
    let mut pricing = PricingState::from_config(&taker_pricing_config(&config));

    pricing.observe_reference_snapshot(
        &reference_tick(
            TEST_PRICING_SNAPSHOT_NEWER_REFERENCE_TS_MS,
            TEST_PRICING_SNAPSHOT_NEWER_REFERENCE_PRICE,
        ),
        config.lead_agreement_min_corr,
        config.lead_jitter_max_ms,
    );
    pricing.observe_reference_snapshot(
        &reference_tick(
            TEST_PRICING_SNAPSHOT_STALE_REFERENCE_TS_MS,
            TEST_PRICING_SNAPSHOT_STALE_REFERENCE_PRICE,
        ),
        config.lead_agreement_min_corr,
        config.lead_jitter_max_ms,
    );

    assert_eq!(
        pricing.last_reference_fair_value,
        Some(TEST_PRICING_SNAPSHOT_NEWER_REFERENCE_PRICE)
    );
    assert_eq!(
        pricing.last_reference_observed_ts_ms,
        Some(TEST_PRICING_SNAPSHOT_NEWER_REFERENCE_TS_MS)
    );
}

#[test]
fn pricing_state_reference_snapshot_processes_signal_candidates_when_fair_value_is_stale() {
    let config = test_strategy().config.clone();
    let mut pricing = PricingState::from_config(&taker_pricing_config(&config));
    let signal_venue = std::any::type_name::<PricingState>();

    pricing.observe_reference_snapshot(
        &reference_tick(
            TEST_PRICING_SNAPSHOT_NEWER_REFERENCE_TS_MS,
            TEST_PRICING_SNAPSHOT_NEWER_REFERENCE_PRICE,
        ),
        config.lead_agreement_min_corr,
        config.lead_jitter_max_ms,
    );
    pricing.observe_reference_snapshot(
        &ReferenceSnapshot {
            ts_ms: TEST_PRICING_SNAPSHOT_STALE_REFERENCE_TS_MS,
            topic: std::any::type_name::<ReferenceSnapshot>().to_string(),
            fair_value: Some(TEST_PRICING_SNAPSHOT_MISMATCHED_STALE_REFERENCE_PRICE),
            confidence: 1.0,
            venues: vec![orderbook_venue(
                signal_venue,
                TEST_PRICING_SNAPSHOT_FAST_WEIGHT,
                TEST_PRICING_SNAPSHOT_FRESH_SIGNAL_PRICE,
                TEST_PRICING_SNAPSHOT_FRESH_SIGNAL_TS_MS,
            )],
        },
        config.lead_agreement_min_corr,
        config.lead_jitter_max_ms,
    );

    assert_eq!(
        pricing.last_reference_fair_value,
        Some(TEST_PRICING_SNAPSHOT_NEWER_REFERENCE_PRICE)
    );
    assert_eq!(
        pricing.last_reference_observed_ts_ms,
        Some(TEST_PRICING_SNAPSHOT_NEWER_REFERENCE_TS_MS)
    );
    assert_eq!(
        pricing.fast_spot,
        Some(fast_spot(
            signal_venue,
            TEST_PRICING_SNAPSHOT_FRESH_SIGNAL_PRICE,
            TEST_PRICING_SNAPSHOT_FRESH_SIGNAL_TS_MS,
        ))
    );
    assert!(!pricing.fast_venue_incoherent);
}

#[test]
fn pricing_state_requires_reference_anchor_for_fast_spot_selection() {
    let config = test_strategy().config.clone();
    let mut pricing = PricingState::from_config(&taker_pricing_config(&config));

    pricing.observe_reference_snapshot(
        &ReferenceSnapshot {
            ts_ms: 1_000,
            topic: "platform.reference.test.spot".to_string(),
            fair_value: None,
            confidence: 1.0,
            venues: vec![orderbook_venue("bybit", 0.9, 3_102.0, 1_000)],
        },
        config.lead_agreement_min_corr,
        config.lead_jitter_max_ms,
    );

    assert_eq!(pricing.spot_price(), None);
    assert_eq!(pricing.last_lead_gap_probability, None);
    assert_eq!(pricing.last_jitter_penalty_probability, None);
    assert_eq!(pricing.last_lead_agreement_corr, None);
}

#[test]
fn pricing_state_applies_lead_quality_thresholds() {
    let mut config = test_strategy().config.clone();
    config.lead_agreement_min_corr = 0.9999;
    let mut pricing = PricingState::from_config(&taker_pricing_config(&config));

    let snapshot = ReferenceSnapshot {
        ts_ms: 1_000,
        topic: "platform.reference.test.spot".to_string(),
        fair_value: Some(3_100.0),
        confidence: 1.0,
        venues: vec![
            oracle_venue("reference", 1.0, 3_100.0, 1_000),
            orderbook_venue("bybit", 0.9, 3_102.0, 1_000),
        ],
    };

    pricing.observe_reference_snapshot(
        &snapshot,
        config.lead_agreement_min_corr,
        config.lead_jitter_max_ms,
    );

    assert!(pricing.fast_spot.is_none());
    assert!(pricing.fast_venue_incoherent);
    assert_eq!(pricing.spot_price(), None);
    assert_eq!(pricing.last_reference_fair_value, Some(3_100.0));
}

#[test]
fn pricing_state_clears_fast_spot_when_no_fast_venue_remains() {
    let config = test_strategy().config.clone();
    let mut pricing = PricingState::from_config(&taker_pricing_config(&config));

    pricing.observe_reference_snapshot(
        &ReferenceSnapshot {
            ts_ms: 1_000,
            topic: "platform.reference.test.spot".to_string(),
            fair_value: Some(3_100.0),
            confidence: 1.0,
            venues: vec![
                oracle_venue("reference", 1.0, 3_100.0, 1_000),
                orderbook_venue("bybit", 0.9, 3_102.0, 1_000),
            ],
        },
        config.lead_agreement_min_corr,
        config.lead_jitter_max_ms,
    );
    assert_eq!(pricing.spot_price(), Some(3_102.0));

    pricing.observe_reference_snapshot(
        &ReferenceSnapshot {
            ts_ms: 1_100,
            topic: "platform.reference.test.spot".to_string(),
            fair_value: Some(3_101.0),
            confidence: 1.0,
            venues: vec![oracle_venue("reference", 1.0, 3_101.0, 1_100)],
        },
        config.lead_agreement_min_corr,
        config.lead_jitter_max_ms,
    );

    assert!(pricing.fast_spot.is_none());
    assert_eq!(pricing.spot_price(), None);
    assert_eq!(pricing.last_reference_fair_value, Some(3_101.0));
}

#[test]
fn selected_realized_vol_for_candidate_falls_closed_when_state_is_missing() {
    let config = test_strategy().config.clone();
    let pricing = PricingState::from_config(&taker_pricing_config(&config));

    let estimator =
        pricing.selected_realized_vol_for_candidate(&lead_signal("bybit", 0, 0, 1.0, 1.0, 0.01));

    assert!(estimator.last_ready_vol.is_none());
    assert_eq!(estimator.current_vol_at(1_000), None);
}

#[test]
fn realized_vol_warms_across_lead_venue_switches_when_each_venue_has_history() {
    let mut strategy = ready_to_trade_strategy();
    strategy.config.vol_min_observations = 3;
    strategy.pricing = PricingState::from_config(&taker_pricing_config(&strategy.config));

    for (ts_ms, venue_name, fair_value, fast_price) in [
        (1_000, "bybit", 3_100.0, 3_100.0),
        (1_100, "okx", 3_100.2, 3_100.2),
        (2_000, "bybit", 3_101.0, 3_101.0),
        (2_100, "okx", 3_101.2, 3_101.2),
        (3_000, "bybit", 3_102.0, 3_102.0),
        (3_100, "okx", 3_102.2, 3_102.2),
        (4_000, "bybit", 3_103.0, 3_103.0),
    ] {
        strategy.observe_reference_snapshot(&ReferenceSnapshot {
            ts_ms,
            topic: "platform.reference.test.spot".to_string(),
            fair_value: Some(fair_value),
            confidence: 1.0,
            venues: vec![
                oracle_venue("reference", 1.0, fair_value, ts_ms),
                orderbook_venue(venue_name, 0.9, fast_price, ts_ms),
            ],
        });
    }

    assert_eq!(
        strategy.pricing.fast_spot,
        Some(fast_spot("bybit", 3_103.0, 4_000))
    );
    assert!(
        strategy.current_realized_vol_at(4_000).is_some(),
        "selected venue should be able to reuse its own warmed history across lead switches"
    );
}

#[test]
fn realized_vol_warms_for_eligible_nonlead_candidates_before_selection() {
    let mut strategy = ready_to_trade_strategy();
    strategy.config.vol_min_observations = 2;
    strategy.config.lead_agreement_min_corr = 0.999;
    strategy.pricing = PricingState::from_config(&taker_pricing_config(&strategy.config));

    for (ts_ms, fair_value, bybit_price, okx_price) in [
        (1_000, 3_100.0, 3_100.0, 3_100.3),
        (2_000, 3_101.0, 3_101.0, 3_101.3),
        (3_000, 3_102.0, 3_102.0, 3_102.3),
        (4_000, 3_103.0, 3_103.0, 3_103.3),
    ] {
        strategy.observe_reference_snapshot(&ReferenceSnapshot {
            ts_ms,
            topic: "platform.reference.test.spot".to_string(),
            fair_value: Some(fair_value),
            confidence: 1.0,
            venues: vec![
                oracle_venue("reference", 1.0, fair_value, ts_ms),
                orderbook_venue("bybit", 0.9, bybit_price, ts_ms),
                orderbook_venue("okx", 0.8, okx_price, ts_ms),
            ],
        });
    }

    assert_eq!(
        strategy.pricing.fast_spot,
        Some(fast_spot("bybit", 3_103.0, 4_000))
    );
    assert!(
        strategy
            .pricing
            .realized_vol_by_venue
            .get("okx")
            .is_some_and(|estimator| estimator.current_vol_at(4_000).is_some()),
        "eligible non-lead venues should keep warming their own realized-vol state"
    );

    strategy.observe_reference_snapshot(&ReferenceSnapshot {
        ts_ms: 5_000,
        topic: "platform.reference.test.spot".to_string(),
        fair_value: Some(3_104.0),
        confidence: 1.0,
        venues: vec![
            oracle_venue("reference", 1.0, 3_104.0, 5_000),
            orderbook_venue("okx", 0.8, 3_104.3, 5_000),
        ],
    });

    assert_eq!(
        strategy.pricing.fast_spot,
        Some(fast_spot("okx", 3_104.3, 5_000))
    );
    assert!(
        strategy.current_realized_vol_at(5_000).is_some(),
        "an eligible venue should be ready immediately once it becomes the selected lead"
    );
}

#[test]
fn realized_vol_does_not_prewarm_ineligible_nonlead_candidates() {
    let mut strategy = ready_to_trade_strategy();
    strategy.config.vol_min_observations = 2;
    strategy.config.lead_agreement_min_corr = 0.999;
    strategy.pricing = PricingState::from_config(&taker_pricing_config(&strategy.config));

    for (ts_ms, fair_value, bybit_price, okx_price) in [
        (1_000, 3_100.0, 3_100.0, 3_000.0),
        (2_000, 3_101.0, 3_101.0, 3_001.0),
        (3_000, 3_102.0, 3_102.0, 3_002.0),
    ] {
        strategy.observe_reference_snapshot(&ReferenceSnapshot {
            ts_ms,
            topic: "platform.reference.test.spot".to_string(),
            fair_value: Some(fair_value),
            confidence: 1.0,
            venues: vec![
                oracle_venue("reference", 1.0, fair_value, ts_ms),
                orderbook_venue("bybit", 0.9, bybit_price, ts_ms),
                orderbook_venue("okx", 0.8, okx_price, ts_ms),
            ],
        });
    }

    assert!(
        !strategy.pricing.realized_vol_by_venue.contains_key("okx"),
        "non-eligible venues should not warm in the background"
    );

    strategy.observe_reference_snapshot(&ReferenceSnapshot {
        ts_ms: 4_000,
        topic: "platform.reference.test.spot".to_string(),
        fair_value: Some(3_103.0),
        confidence: 1.0,
        venues: vec![
            oracle_venue("reference", 1.0, 3_103.0, 4_000),
            orderbook_venue("okx", 0.8, 3_103.0, 4_000),
        ],
    });

    assert_eq!(
        strategy.pricing.fast_spot,
        Some(fast_spot("okx", 3_103.0, 4_000))
    );
    assert!(
        strategy.current_realized_vol_at(4_000).is_none(),
        "a venue that was previously ineligible should still cold-start when it first becomes eligible"
    );
}

#[test]
fn realized_vol_does_not_borrow_ready_state_from_a_different_venue() {
    let mut strategy = ready_to_trade_strategy();
    strategy.config.vol_min_observations = 2;
    strategy.pricing = PricingState::from_config(&taker_pricing_config(&strategy.config));

    for (ts_ms, fair_value, fast_price) in [
        (1_000, 3_100.0, 3_100.0),
        (2_000, 3_101.0, 3_101.0),
        (3_000, 3_102.0, 3_102.0),
    ] {
        strategy.observe_reference_snapshot(&ReferenceSnapshot {
            ts_ms,
            topic: "platform.reference.test.spot".to_string(),
            fair_value: Some(fair_value),
            confidence: 1.0,
            venues: vec![
                oracle_venue("reference", 1.0, fair_value, ts_ms),
                orderbook_venue("bybit", 0.9, fast_price, ts_ms),
            ],
        });
    }

    assert!(
        strategy.current_realized_vol_at(3_000).is_some(),
        "bybit should be warmed before the lead venue changes"
    );

    strategy.observe_reference_snapshot(&ReferenceSnapshot {
        ts_ms: 3_100,
        topic: "platform.reference.test.spot".to_string(),
        fair_value: Some(3_102.2),
        confidence: 1.0,
        venues: vec![
            oracle_venue("reference", 1.0, 3_102.2, 3_100),
            orderbook_venue("okx", 0.9, 3_102.2, 3_100),
        ],
    });

    assert_eq!(
        strategy.pricing.fast_spot,
        Some(fast_spot("okx", 3_102.2, 3_100))
    );
    assert!(
        strategy.current_realized_vol_at(3_100).is_none(),
        "selected venue should not inherit warmed vol from another venue"
    );
}

#[test]
fn realized_vol_resets_per_venue_after_gap_even_if_other_venue_keeps_warming() {
    let mut strategy = ready_to_trade_strategy();
    strategy.config.vol_min_observations = 1;
    strategy.config.vol_gap_reset_secs = 1;
    strategy.config.vol_bridge_valid_secs = 10;
    strategy.config.lead_jitter_max_ms = 10_000;
    strategy.pricing = PricingState::from_config(&taker_pricing_config(&strategy.config));

    for (ts_ms, venue_name, fair_value, fast_price) in [
        (1_000, "bybit", 3_100.0, 3_100.0),
        (1_500, "bybit", 3_101.0, 3_101.0),
        (2_600, "okx", 3_101.5, 3_101.5),
        (3_100, "okx", 3_102.0, 3_102.0),
    ] {
        strategy.observe_reference_snapshot(&ReferenceSnapshot {
            ts_ms,
            topic: "platform.reference.test.spot".to_string(),
            fair_value: Some(fair_value),
            confidence: 1.0,
            venues: vec![
                oracle_venue("reference", 1.0, fair_value, ts_ms),
                orderbook_venue(venue_name, 0.9, fast_price, ts_ms),
            ],
        });
    }

    assert_eq!(
        strategy.pricing.fast_spot,
        Some(fast_spot("okx", 3_102.0, 3_100))
    );
    assert!(
        strategy.current_realized_vol_at(3_100).is_some(),
        "okx should warm independently while bybit is absent"
    );

    strategy.observe_reference_snapshot(&ReferenceSnapshot {
        ts_ms: 4_201,
        topic: "platform.reference.test.spot".to_string(),
        fair_value: Some(3_102.5),
        confidence: 1.0,
        venues: vec![
            oracle_venue("reference", 1.0, 3_102.5, 4_201),
            orderbook_venue("bybit", 0.9, 3_102.5, 4_201),
        ],
    });

    assert_eq!(
        strategy.pricing.fast_spot,
        Some(fast_spot("bybit", 3_102.5, 4_201))
    );
    assert!(
        strategy.current_realized_vol_at(4_201).is_none(),
        "bybit should reset after its own gap instead of bridging stale or other-venue vol"
    );
}

#[test]
fn pricing_state_reports_realized_vol_source_during_bridge_without_fast_spot() {
    let config = test_strategy().config.clone();
    let mut pricing = PricingState::from_config(&taker_pricing_config(&config));
    pricing.realized_vol_source_venue = Some("bybit".to_string());
    pricing.realized_vol.last_ready_vol = Some(1.5);
    pricing.realized_vol.last_ready_ts_ms = Some(1_200);

    assert_eq!(
        pricing.current_realized_vol_source_at(1_300),
        (Some("bybit".to_string()), Some(1_200))
    );
    assert_eq!(pricing.current_realized_vol_source_at(12_201), (None, None));
}

#[test]
fn entry_evaluation_log_fields_fail_closed_without_fast_spot() {
    let mut strategy = ready_to_trade_strategy_with_live_fees(Decimal::ZERO, Decimal::ZERO);
    strategy.pricing.fast_spot = None;
    strategy.pricing.last_reference_fair_value = Some(3_101.0);
    strategy.pricing.realized_vol.last_ready_vol = Some(2.5);
    strategy.pricing.realized_vol.last_ready_ts_ms = Some(1_200);
    strategy.pricing.realized_vol_source_venue = Some("bybit".to_string());

    let submission = strategy.entry_submission_decision_at(1_200);
    let fields = strategy.entry_evaluation_log_fields_at(1_200, &submission);

    assert_eq!(fields.spot_venue_name, None);
    assert_eq!(fields.spot_price, None);
    assert_eq!(
        fields.pricing_blocked_by,
        vec![EntryPricingBlockReason::SpotPriceMissing]
    );
    assert_eq!(fields.realized_vol, Some(2.5));
    assert_eq!(fields.realized_vol_source_venue.as_deref(), Some("bybit"));
    assert_eq!(fields.realized_vol_source_ts_ms, Some(1_200));
}

#[test]
fn entry_evaluation_blocks_when_realized_vol_is_not_ready() {
    let mut strategy = ready_to_trade_strategy_with_live_fees(Decimal::ZERO, Decimal::ZERO);
    strategy.pricing.fast_spot = Some(fast_spot("bybit", 3_101.0, 1_200));
    strategy.pricing.realized_vol.last_ready_vol = None;
    strategy.pricing.realized_vol.last_ready_ts_ms = None;

    let decision = strategy.entry_evaluation_at(1_200);

    assert!(decision.gate.blocked_by.is_empty());
    assert_eq!(
        decision.pricing_blocked_by,
        vec![EntryPricingBlockReason::RealizedVolNotReady]
    );
}

#[test]
fn live_fair_probability_is_computed_from_strategy_state_once_vol_warms() {
    let mut strategy = ready_to_trade_strategy();
    strategy.config.vol_min_observations = 3;
    strategy.pricing = PricingState::from_config(&taker_pricing_config(&strategy.config));

    for (ts_ms, fair_value, fast_spot_price) in [
        (1_000, 3_100.0, 3_100.0),
        (2_000, 3_101.0, 3_101.5),
        (3_000, 3_102.0, 3_103.0),
        (4_000, 3_103.0, 3_104.0),
    ] {
        strategy.observe_reference_snapshot(&ReferenceSnapshot {
            ts_ms,
            topic: "platform.reference.test.spot".to_string(),
            fair_value: Some(fair_value),
            confidence: 1.0,
            venues: vec![orderbook_venue("bybit", 0.9, fast_spot_price, ts_ms)],
        });
    }

    let fair_probability = strategy
        .current_fair_probability_up_at(4_000)
        .expect("warmed pricing state should produce fair probability");
    assert!(fair_probability > 0.5);

    let decision = strategy.entry_evaluation_at(4_000);
    assert!(decision.pricing_blocked_by.is_empty());
}

#[test]
fn live_scaled_min_edge_uses_theta_scaler_near_expiry() {
    let mut strategy = ready_to_trade_strategy();
    strategy.config.edge_threshold_basis_points = 10;
    strategy.config.theta_decay_factor = 1.5;

    let early = strategy
        .current_scaled_min_edge_bps_at(1_000)
        .expect("theta-scaled threshold should compute");
    let late = strategy
        .current_scaled_min_edge_bps_at(591_000)
        .expect("theta-scaled threshold should compute");

    assert!((early - 10.0).abs() < 1e-9);
    assert!(late > early);
}

#[test]
fn interval_open_requires_source_bound_price_to_beat_before_reference_quote_warms_market() {
    let mut strategy = test_strategy();
    strategy.apply_selection_snapshot(active_snapshot_with_start("MKT-1", 1_000));

    strategy.observe_reference_quote(&fast_spot("reference", 3_101.0, 1_000));

    assert_eq!(strategy.active.interval_open, None);
    assert_eq!(strategy.active.last_reference_ts_ms, None);
    assert_eq!(strategy.active.warmup_count, INITIAL_COUNTER_U64);
}

#[test]
fn entry_evaluation_uses_price_adjusted_fee_bps_not_cached_base_fee_rate() {
    let (mut strategy, fee_provider) =
        ready_to_trade_strategy_with_recording_fees(Decimal::from(1000), Decimal::from(1000));
    fee_provider.set_entry_fee_bps(
        "condition-MKT-1-MKT-1-UP.POLYMARKET",
        Decimal::from_str("511.111111111111").expect("test decimal should parse"),
    );
    fee_provider.set_entry_fee_bps(
        "condition-MKT-1-MKT-1-DOWN.POLYMARKET",
        Decimal::from_str("182.027027027027").expect("test decimal should parse"),
    );
    register_test_strategy_with_active_instruments(&mut strategy);

    let decision = strategy.entry_submission_decision_at(1_200);
    let fields = strategy.entry_evaluation_log_fields_at(1_200, &decision);

    assert_eq!(fields.up_fee_bps, Some(511.111111111111));
    assert_eq!(fields.down_fee_bps, Some(182.027027027027));
}

#[test]
fn task4_lead_arbitration_uses_composite_score_over_fixed_precedence() {
    let candidates = vec![
        lead_signal("younger_but_weaker", 10, 10, 0.81, 1.0, 0.01),
        lead_signal("older_but_stronger", 20, 10, 0.99, 4.0, 0.01),
    ];

    let selected =
        arbitrate_lead_reference(&candidates, 0.80, 25).expect("winner should be eligible");

    assert_eq!(selected.venue_name, "older_but_stronger");
}

#[test]
fn task4_lead_arbitration_uses_reference_when_no_fast_venue_is_eligible() {
    let candidates = vec![
        lead_signal("too_noisy", 20, 300, 0.95, 4.0, 0.01),
        lead_signal("disagrees", 20, 20, 0.79, 4.0, 0.01),
        lead_signal("weightless", 20, 20, 0.95, 0.0, 0.01),
    ];

    let selected = arbitrate_lead_reference(&candidates, 0.80, 250);

    assert!(selected.is_none());
}

#[test]
fn task4_lead_arbitration_fails_closed_on_exact_score_tie() {
    let candidates = vec![
        lead_signal("lighter", 10, 10, 0.90, 2.0, 0.01),
        lead_signal("heavier", 10, 10, 0.90, 3.0, 0.01),
    ];

    let selected = arbitrate_lead_reference(&candidates, 0.80, 25);

    assert!(selected.is_none());
}

#[test]
fn reference_spot_spike_sets_cooldown_and_blocks_then_allows_entry() {
    let mut strategy = ready_to_trade_strategy();
    // Threshold from the test config fixture.
    assert_eq!(strategy.config.spike_guard_return_threshold, 0.05);
    assert_eq!(strategy.config.spike_guard_cooldown_secs, 5);

    // Seed a previous reference-spot observation so the next one has a baseline.
    strategy.pricing.fast_spot = Some(fast_spot("bybit", 100.0, 1_000));

    // A jump from 100.0 -> 110.0 is a 10% single-step move, >= the 5% threshold.
    strategy.pricing.observe_signal_quote(
        &fast_spot("bybit", 110.0, 2_000),
        &taker_pricing_config(&strategy.config),
    );

    // Cooldown deadline = observed_ts (2_000ms) + 5s * 1_000ms = 7_000ms.
    assert_eq!(strategy.pricing.spike_until_ms, Some(7_000));

    // Entry is blocked while now_ms < spike_until_ms.
    assert!(
        strategy
            .entry_gate_decision_at(6_999)
            .blocked_by
            .contains(&EntryBlockReason::SpotSpikeCooldown),
        "entry must be blocked before the spike cooldown deadline"
    );
    // Boundary: at the deadline the cooldown has elapsed (now_ms < deadline is false).
    assert!(
        !strategy
            .entry_gate_decision_at(7_000)
            .blocked_by
            .contains(&EntryBlockReason::SpotSpikeCooldown),
        "entry must be allowed once now_ms reaches the spike cooldown deadline"
    );
    assert!(
        !strategy
            .entry_gate_decision_at(7_001)
            .blocked_by
            .contains(&EntryBlockReason::SpotSpikeCooldown),
        "entry must be allowed after the spike cooldown deadline"
    );
}

#[test]
fn sub_threshold_reference_spot_move_does_not_arm_spike_cooldown() {
    let mut strategy = ready_to_trade_strategy();
    strategy.pricing.spike_until_ms = None;
    strategy.pricing.fast_spot = Some(fast_spot("bybit", 100.0, 1_000));

    // A 2% move (100.0 -> 102.0) is below the 5% threshold.
    strategy.pricing.observe_signal_quote(
        &fast_spot("bybit", 102.0, 2_000),
        &taker_pricing_config(&strategy.config),
    );

    assert_eq!(
        strategy.pricing.spike_until_ms, None,
        "sub-threshold move must not arm the spike cooldown"
    );
    assert!(
        !strategy
            .entry_gate_decision_at(2_001)
            .blocked_by
            .contains(&EntryBlockReason::SpotSpikeCooldown)
    );
}

#[test]
fn spike_detection_requires_a_valid_previous_observation() {
    let mut strategy = ready_to_trade_strategy();
    strategy.pricing.fast_spot = None;
    strategy.pricing.spike_until_ms = None;

    // First observation has no baseline; a spike cannot be inferred.
    strategy.pricing.observe_signal_quote(
        &fast_spot("bybit", 110.0, 2_000),
        &taker_pricing_config(&strategy.config),
    );

    assert_eq!(
        strategy.pricing.spike_until_ms, None,
        "no previous observation means no spike"
    );
}

#[test]
fn spike_cooldown_deadline_only_extends_never_retracts() {
    // The spike cooldown is a fail-closed safety gate: an out-of-order spike
    // quote carrying an earlier timestamp must never shorten an active
    // cooldown (which would prematurely re-enable entry during volatility).
    let mut strategy = ready_to_trade_strategy();

    // Pre-arm an active cooldown deadline at 7_000ms with a seeded baseline.
    strategy.pricing.spike_until_ms = Some(7_000);
    strategy.pricing.fast_spot = Some(fast_spot("bybit", 100.0, 1_000));

    // Out-of-order spike: 100 -> 130 (30% >= 5% threshold) at an earlier ts
    // (1_500ms). Its naive deadline 1_500 + 5_000 = 6_500ms is before the
    // active 7_000ms and must not retract it.
    strategy.pricing.observe_signal_quote(
        &fast_spot("bybit", 130.0, 1_500),
        &taker_pricing_config(&strategy.config),
    );
    assert_eq!(
        strategy.pricing.spike_until_ms,
        Some(7_000),
        "an out-of-order spike must not shorten an active cooldown deadline"
    );

    // A later spike further into the future extends the deadline forward.
    // Reset the baseline so detection is independent of eligibility chaining.
    strategy.pricing.fast_spot = Some(fast_spot("bybit", 100.0, 1_000));
    strategy.pricing.observe_signal_quote(
        &fast_spot("bybit", 130.0, 4_000),
        &taker_pricing_config(&strategy.config),
    );
    assert_eq!(
        strategy.pricing.spike_until_ms,
        Some(9_000),
        "a later spike (ts 4_000 + 5s) must extend the deadline to 9_000ms"
    );
}

#[test]
fn task5_exit_decision_uses_hysteresis_boundary_and_fails_closed() {
    assert_eq!(
        evaluate_exit_decision(Some(12.0), Some(11.0), 1.0),
        ExitDecision::Exit
    );
    assert_eq!(
        evaluate_exit_decision(Some(12.0), Some(10.5), 1.0),
        ExitDecision::Hold
    );
    assert_eq!(
        evaluate_exit_decision(None, Some(10.0), 1.0),
        ExitDecision::ExitFailClosed
    );
    assert_eq!(
        evaluate_exit_decision(Some(12.0), Some(f64::NAN), 1.0),
        ExitDecision::ExitFailClosed
    );
}

#[test]
fn task6_entry_evaluation_blocks_when_realized_vol_is_not_ready() {
    let mut strategy = ready_to_trade_strategy_with_live_fees(Decimal::ZERO, Decimal::ZERO);
    strategy.pricing.fast_spot = Some(fast_spot("bybit", 3_101.0, 1_200));
    strategy.pricing.realized_vol.last_ready_vol = None;
    strategy.pricing.realized_vol.last_ready_ts_ms = None;

    let decision = strategy.entry_evaluation_at(1_200);

    assert!(decision.gate.blocked_by.is_empty());
    assert_eq!(
        decision.pricing_blocked_by,
        vec![EntryPricingBlockReason::RealizedVolNotReady]
    );
    assert_eq!(decision.selected_side, None);
}

#[test]
fn task6_entry_evaluation_computes_both_side_evs_from_live_state() {
    let mut strategy = ready_to_trade_strategy_with_live_fees(Decimal::ZERO, Decimal::ZERO);
    strategy.pricing.fast_spot = Some(fast_spot("bybit", 3_100.4, 1_200));
    strategy.pricing.realized_vol.last_ready_vol = Some(2.5);
    strategy.pricing.realized_vol.last_ready_ts_ms = Some(1_200);

    let decision = strategy.entry_evaluation_at(1_200);

    assert!(decision.gate.blocked_by.is_empty());
    assert!(decision.pricing_blocked_by.is_empty());
    assert!(
        decision
            .fair_probability_up
            .is_some_and(|value| value > 0.5),
        "live pricing should infer an up edge from spot above strike"
    );
    assert!(decision.up_worst_case_ev_bps.is_some());
    assert!(decision.down_worst_case_ev_bps.is_some());
    assert!(
        decision
            .expected_ev_per_notional
            .is_some_and(|value| value > 0.0)
    );
    assert!(
        decision
            .book_impact_cap_notional
            .is_some_and(|value| value > 0.0)
    );
    assert!(decision.sized_notional.is_some_and(|value| value > 0.0));
    assert_eq!(decision.selected_side, Some(OutcomeSide::Up));
}

#[test]
fn task6_entry_evaluation_uses_live_uncertainty_band_probability() {
    let mut strategy =
        ready_to_trade_strategy_with_live_fees(Decimal::new(250, 2), Decimal::new(250, 2));
    strategy.pricing.fast_spot = Some(fast_spot("bybit", 3_100.4, 1_200));
    strategy.pricing.realized_vol.last_ready_vol = Some(2.5);
    strategy.pricing.realized_vol.last_ready_ts_ms = Some(1_200);
    strategy.pricing.last_lead_gap_probability = Some(0.02);
    strategy.pricing.last_jitter_penalty_probability = Some(0.01);

    let decision = strategy.entry_evaluation_at(1_200);

    assert!(decision.pricing_blocked_by.is_empty());
    assert!(
        decision
            .uncertainty_band_probability
            .is_some_and(|value| value > 0.0)
    );
}

#[test]
fn task6_entry_evaluation_requires_live_uncertainty_components() {
    let mut strategy =
        ready_to_trade_strategy_with_live_fees(Decimal::new(250, 2), Decimal::new(250, 2));
    strategy.pricing.fast_spot = Some(fast_spot("bybit", 3_100.4, 1_200));
    strategy.pricing.realized_vol.last_ready_vol = Some(2.5);
    strategy.pricing.realized_vol.last_ready_ts_ms = Some(1_200);
    strategy.pricing.last_lead_gap_probability = None;
    strategy.pricing.last_jitter_penalty_probability = None;

    let decision = strategy.entry_evaluation_at(1_200);

    assert_eq!(
        decision.pricing_blocked_by,
        vec![EntryPricingBlockReason::UncertaintyBandUnavailable]
    );
    assert_eq!(decision.uncertainty_band_probability, None);
}

#[test]
fn task6_entry_evaluation_applies_theta_scaled_threshold_at_boundary() {
    let mut strategy = ready_to_trade_strategy_with_live_fees(Decimal::ZERO, Decimal::ZERO);
    strategy.pricing.fast_spot = Some(fast_spot("bybit", 3_120.0, 1_200));
    strategy.pricing.realized_vol.last_ready_vol = Some(2.5);
    strategy.pricing.realized_vol.last_ready_ts_ms = Some(1_200);
    strategy.pricing.realized_vol.bridge_valid_ms = 1_000_000;
    strategy.config.edge_threshold_basis_points = 2_000;

    let baseline = strategy.entry_evaluation_at(1_200);
    assert_eq!(baseline.selected_side, Some(OutcomeSide::Up));

    strategy.config.theta_decay_factor = 100.0;
    strategy.active.last_reference_ts_ms = Some(291_000);
    strategy.pricing.fast_spot = Some(fast_spot("bybit", 3_120.0, 291_000));
    strategy.pricing.realized_vol.last_ready_ts_ms = Some(291_000);
    let near_expiry = strategy.entry_evaluation_at(291_000);

    assert!(near_expiry.gate.blocked_by.is_empty());
    assert!(near_expiry.pricing_blocked_by.is_empty());
    assert!(near_expiry.up_worst_case_ev_bps.is_some());
    assert!(near_expiry.min_worst_case_ev_bps.is_some());
    assert_eq!(near_expiry.selected_side, None);
    assert!(
        near_expiry
            .min_worst_case_ev_bps
            .zip(near_expiry.up_worst_case_ev_bps)
            .is_some_and(|(threshold, up_ev)| threshold >= up_ev),
        "theta-scaled threshold should close the entry boundary near expiry"
    );
}

#[test]
fn entry_evaluation_log_fields_capture_parameters_and_omissions() {
    let mut strategy = ready_to_trade_strategy_with_live_fees(Decimal::ZERO, Decimal::ZERO);
    strategy.observe_reference_snapshot(&ReferenceSnapshot {
        ts_ms: 1_200,
        topic: "platform.reference.test.spot".to_string(),
        fair_value: Some(3_100.5),
        confidence: 1.0,
        venues: vec![
            oracle_venue("reference", 1.0, 3_100.5, 1_200),
            orderbook_venue("bybit", 0.9, 3_101.0, 1_200),
        ],
    });
    strategy.pricing.realized_vol.last_ready_vol = Some(2.5);
    strategy.pricing.realized_vol.last_ready_ts_ms = Some(1_200);

    let evaluation = strategy.entry_evaluation_at(1_200);
    let submission = strategy.entry_submission_decision_at(1_200);
    let fields = strategy.entry_evaluation_log_fields_at(1_200, &submission);

    assert_eq!(fields.market_id.as_deref(), Some("MKT-1"));
    assert_eq!(fields.phase, SelectionPhase::Active);
    assert_eq!(fields.spot_venue_name.as_deref(), Some("bybit"));
    assert_eq!(fields.spot_price, Some(3_101.0));
    assert_eq!(fields.reference_fair_value, Some(3_100.5));
    assert_eq!(fields.interval_open, Some(3_100.0));
    assert_eq!(fields.realized_vol, Some(2.5));
    assert_eq!(fields.realized_vol_source_venue.as_deref(), Some("bybit"));
    assert_eq!(fields.realized_vol_source_ts_ms, Some(1_200));
    assert_eq!(fields.fair_probability_up, evaluation.fair_probability_up);
    assert_eq!(fields.selected_side, evaluation.selected_side);
    assert!(fields.uncertainty_band_probability.is_some());
    assert!(fields.uncertainty_band_live);
    assert_eq!(
        fields.uncertainty_band_reason,
        "derived_from_lead_gap_jitter_time_and_fee"
    );
    assert!(fields.lead_quality_policy_applied);
    assert!(
        fields
            .expected_ev_per_notional
            .is_some_and(|value| value > 0.0)
    );
    assert_eq!(
        fields.maximum_position_notional,
        strategy.config.maximum_position_notional
    );
    assert_eq!(fields.risk_lambda, strategy.config.risk_lambda);
    assert_eq!(
        fields.book_impact_cap_bps,
        strategy.config.book_impact_cap_bps
    );
    assert!(
        fields
            .book_impact_cap_notional
            .is_some_and(|value| value > 0.0)
    );
    assert!(fields.sized_notional.is_some_and(|value| value > 0.0));
    assert!(!fields.final_fee_amount_known);
}

#[test]
fn task6_exit_decision_requires_live_uncertainty_components() {
    let mut strategy = ready_to_trade_strategy_with_live_fees(Decimal::ZERO, Decimal::ZERO);
    let open_position = OpenPositionState {
        market_id: Some("MKT-1".to_string()),
        instrument_id: strategy.active.books.up.instrument_id.unwrap(),
        position_id: PositionId::from("P-UP-MISSING-UNCERTAINTY"),
        outcome_side: Some(OutcomeSide::Up),
        outcome_fees: strategy.active.outcome_fees.clone(),
        historical_entry_fee_bps: Some(0.0),
        entry_order_side: OrderSide::Buy,
        side: PositionSide::Long,
        quantity: Quantity::new(10.0, 2),
        avg_px_open: 0.450,
        interval_open: Some(3_100.0),
        selection_published_at_ms: Some(1_000),
        seconds_to_expiry_at_selection: Some(300),
        book: strategy.active.books.up.clone(),
    };
    set_managed_position(
        &mut strategy,
        open_position,
        ManagedPositionOrigin::StrategyEntry,
    );
    strategy.pricing.fast_spot = Some(fast_spot("bybit", 3_099.5, 1_200));
    strategy.pricing.realized_vol.last_ready_vol = Some(2.5);
    strategy.pricing.realized_vol.last_ready_ts_ms = Some(1_200);
    strategy.pricing.last_lead_gap_probability = None;
    strategy.pricing.last_jitter_penalty_probability = None;

    let decision = strategy.exit_submission_decision_at(1_200);

    assert_eq!(decision.evaluation.hold_ev_bps, None);
    assert!(decision.evaluation.exit_ev_bps.is_some());
    assert_eq!(
        decision.evaluation.exit_decision,
        Some(ExitDecision::ExitFailClosed)
    );
    assert_eq!(decision.order_side, Some(OrderSide::Sell));
    assert_eq!(
        decision.instrument_id,
        strategy.active.books.up.instrument_id
    );
    assert_eq!(decision.blocked_reason, None);
}
