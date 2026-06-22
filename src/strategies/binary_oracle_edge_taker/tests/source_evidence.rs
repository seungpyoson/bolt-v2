#![cfg(test)]

use super::*;

const TEST_SURFACE_ID: &str = "<surface_id>";
const TEST_SOURCE_ID: &str = "<SOURCE_ID_A>";
const TEST_SOURCE_ID_B: &str = "<SOURCE_ID_B>";
const TEST_TRADE_SOURCE_ID: &str = "<SOURCE_ID_TRADE>";
const TEST_RV_INSTRUMENT_ID: &str = "<INSTRUMENT_ID_A>.<DATA_CLIENT_ID>";

fn test_realized_volatility_engine_config()
-> crate::bolt_v3_realized_volatility::RealizedVolEngineConfig {
    crate::bolt_v3_realized_volatility::RealizedVolEngineConfig {
        surface_id: TEST_SURFACE_ID.to_string(),
        window_ms: 4_000,
        sampling_interval_ms: 1_000,
        min_ready_sources: 1,
        max_source_age_ms: 500,
        max_event_receive_lag_ms: 250,
        max_inter_sample_gap_ms: 2_000,
        min_coverage_ratio: 0.75,
        max_cross_source_dispersion: 0.50,
        seconds_per_annum: 31_536_000.0,
        aggregation: crate::bolt_v3_realized_volatility::RealizedVolAggregation::UpperQuantile {
            quantile: 1.0,
        },
        estimator: crate::bolt_v3_realized_volatility::RealizedVolEstimatorConfig::measured(),
        sources: vec![
            crate::bolt_v3_realized_volatility::RealizedVolSourceConfig {
                source_id: TEST_SOURCE_ID.to_string(),
                data_client_id: "<DATA_CLIENT_ID>".to_string(),
                instrument_id: TEST_RV_INSTRUMENT_ID.to_string(),
                source_class: crate::bolt_v3_realized_volatility::RealizedVolSourceClass::SpotQuote,
                sample_kind: crate::bolt_v3_realized_volatility::RealizedVolSampleKind::Midpoint,
                enabled: true,
                counts_toward_quorum: true,
                canonical_base_asset: "<BASE_ASSET>".to_string(),
                canonical_quote_asset: "<QUOTE_ASSET>".to_string(),
            },
        ],
    }
}

fn test_strategy_with_realized_volatility_surface(
    engine_config: crate::bolt_v3_realized_volatility::RealizedVolEngineConfig,
) -> BinaryOracleEdgeTaker {
    let base = test_strategy();
    let mut config = base.config.clone();
    config.realized_volatility_surface_id = TEST_SURFACE_ID.to_string();
    let mut surfaces = std::collections::BTreeMap::new();
    surfaces.insert(TEST_SURFACE_ID.to_string(), engine_config);
    let decision_evidence = std::sync::Arc::new(RecordingDecisionEvidenceWriter);
    let submit_admission = std::sync::Arc::new(
        crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new(decision_evidence.clone()),
    );
    let context = StrategyBuildContext::new(
        RecordingFeeProvider::cold(),
        decision_evidence,
        submit_admission,
        crate::bolt_v3_order_execution::BoltV3OrderExecutionPolicy::live(),
        fixture_execution_venue(),
    )
    .with_realized_volatility_surfaces(surfaces);
    BinaryOracleEdgeTaker::new(config, context)
}

#[test]
fn surfaced_realized_volatility_quote_source_forwards_snapshot_to_pricing() {
    let mut strategy =
        test_strategy_with_realized_volatility_surface(test_realized_volatility_engine_config());

    strategy
        .on_quote(&quote_tick(TEST_RV_INSTRUMENT_ID, 100.0, 102.0, 1_000))
        .expect("configured RV quote source should process");

    let snapshot = strategy
        .pricing
        .latest_realized_vol_snapshot_for_surface(TEST_SURFACE_ID)
        .expect("RV forwarding should publish a pricing snapshot");
    assert_eq!(snapshot.surface_id, TEST_SURFACE_ID);
    assert_eq!(snapshot.source_diagnostics[0].source_id, TEST_SOURCE_ID);
    assert_eq!(snapshot.source_diagnostics[0].raw_sample_count, 1);
}

#[test]
fn surfaced_realized_volatility_forwards_duplicate_stream_bindings() {
    let mut engine_config = test_realized_volatility_engine_config();
    engine_config.sources.push(
        crate::bolt_v3_realized_volatility::RealizedVolSourceConfig {
            source_id: TEST_SOURCE_ID_B.to_string(),
            data_client_id: "<DATA_CLIENT_ID>".to_string(),
            instrument_id: TEST_RV_INSTRUMENT_ID.to_string(),
            source_class: crate::bolt_v3_realized_volatility::RealizedVolSourceClass::SpotQuote,
            sample_kind: crate::bolt_v3_realized_volatility::RealizedVolSampleKind::Midpoint,
            enabled: true,
            counts_toward_quorum: true,
            canonical_base_asset: "<BASE_ASSET>".to_string(),
            canonical_quote_asset: "<QUOTE_ASSET>".to_string(),
        },
    );
    let mut strategy = test_strategy_with_realized_volatility_surface(engine_config);

    strategy
        .on_quote(&quote_tick(TEST_RV_INSTRUMENT_ID, 100.0, 102.0, 1_000))
        .expect("configured RV quote source should process");

    let snapshot = strategy
        .pricing
        .latest_realized_vol_snapshot_for_surface(TEST_SURFACE_ID)
        .expect("RV forwarding should publish a pricing snapshot");
    for source_id in [TEST_SOURCE_ID, TEST_SOURCE_ID_B] {
        let diagnostic = snapshot
            .source_diagnostics
            .iter()
            .find(|diagnostic| diagnostic.source_id == source_id)
            .expect("duplicate stream source diagnostic should exist");
        assert_eq!(
            diagnostic.raw_sample_count, 1,
            "duplicate stream source {source_id} should receive the quote tick"
        );
    }
}

#[test]
fn surfaced_realized_volatility_forwards_disabled_source_observations_for_audit() {
    let mut engine_config = test_realized_volatility_engine_config();
    engine_config.sources.push(
        crate::bolt_v3_realized_volatility::RealizedVolSourceConfig {
            source_id: TEST_SOURCE_ID_B.to_string(),
            data_client_id: "<DATA_CLIENT_ID>".to_string(),
            instrument_id: TEST_RV_INSTRUMENT_ID.to_string(),
            source_class: crate::bolt_v3_realized_volatility::RealizedVolSourceClass::SpotQuote,
            sample_kind: crate::bolt_v3_realized_volatility::RealizedVolSampleKind::Midpoint,
            enabled: false,
            counts_toward_quorum: false,
            canonical_base_asset: "<BASE_ASSET>".to_string(),
            canonical_quote_asset: "<QUOTE_ASSET>".to_string(),
        },
    );
    let mut strategy = test_strategy_with_realized_volatility_surface(engine_config);

    strategy
        .on_quote(&quote_tick(TEST_RV_INSTRUMENT_ID, 100.0, 102.0, 1_000))
        .expect("configured RV quote source should process");

    let snapshot = strategy
        .pricing
        .latest_realized_vol_snapshot_for_surface(TEST_SURFACE_ID)
        .expect("RV forwarding should publish a pricing snapshot");
    let disabled_diagnostic = snapshot
        .source_diagnostics
        .iter()
        .find(|diagnostic| diagnostic.source_id == TEST_SOURCE_ID_B)
        .expect("disabled source diagnostic should exist");
    assert_eq!(
        disabled_diagnostic.status,
        crate::bolt_v3_realized_volatility::RealizedVolSourceStatus::DiagnosticOnly
    );
    assert_eq!(
        disabled_diagnostic.last_rejected_reason,
        Some(crate::bolt_v3_realized_volatility::RealizedVolSourceRejectReason::DisabledSource)
    );
    assert_eq!(
        disabled_diagnostic.rejection_counters.get(
            &crate::bolt_v3_realized_volatility::RealizedVolSourceRejectReason::DisabledSource
        ),
        Some(&1)
    );
}

#[test]
fn realized_volatility_runtime_keeps_disabled_sources_non_subscribable_for_audit_fanout() {
    let mut engine_config = test_realized_volatility_engine_config();
    engine_config.sources.push(
        crate::bolt_v3_realized_volatility::RealizedVolSourceConfig {
            source_id: TEST_SOURCE_ID_B.to_string(),
            data_client_id: "<DATA_CLIENT_ID>".to_string(),
            instrument_id: TEST_RV_INSTRUMENT_ID.to_string(),
            source_class: crate::bolt_v3_realized_volatility::RealizedVolSourceClass::SpotQuote,
            sample_kind: crate::bolt_v3_realized_volatility::RealizedVolSampleKind::Midpoint,
            enabled: false,
            counts_toward_quorum: false,
            canonical_base_asset: "<BASE_ASSET>".to_string(),
            canonical_quote_asset: "<QUOTE_ASSET>".to_string(),
        },
    );
    let mut surfaces = std::collections::BTreeMap::new();
    surfaces.insert(TEST_SURFACE_ID.to_string(), engine_config);
    let runtime =
        crate::bolt_v3_realized_volatility_runtime::RealizedVolSurfaceRuntime::from_configs(
            surfaces,
        )
        .expect("runtime should build");

    assert_eq!(runtime.subscription_requests().len(), 1);
}

#[test]
fn invalid_realized_volatility_runtime_config_rejects_subscriptions() {
    let mut engine_config = test_realized_volatility_engine_config();
    engine_config.sampling_interval_ms = engine_config.window_ms + 1;
    let mut surfaces = std::collections::BTreeMap::new();
    surfaces.insert(TEST_SURFACE_ID.to_string(), engine_config);

    assert!(
        crate::bolt_v3_realized_volatility_runtime::RealizedVolSurfaceRuntime::from_configs(
            surfaces,
        )
        .is_err(),
        "rejected runtime config must not leave source subscriptions behind"
    );
}

#[test]
fn surfaced_realized_volatility_refresh_blocks_when_source_goes_stale() {
    let mut strategy =
        test_strategy_with_realized_volatility_surface(test_realized_volatility_engine_config());

    for (ts_ms, bid, ask) in [
        (1_000, 100.0, 102.0),
        (2_000, 101.0, 103.0),
        (3_000, 102.0, 104.0),
        (4_000, 103.0, 105.0),
    ] {
        strategy
            .on_quote(&quote_tick(TEST_RV_INSTRUMENT_ID, bid, ask, ts_ms))
            .expect("configured RV quote source should process");
    }
    assert!(strategy.current_realized_vol_at(4_000).is_some());

    strategy.refresh_realized_volatility_snapshot_at(4_501);

    assert_eq!(strategy.current_realized_vol_at(4_501), None);
    let snapshot = strategy
        .pricing
        .latest_realized_vol_snapshot_for_surface(TEST_SURFACE_ID)
        .expect("RV refresh should publish a pricing snapshot");
    assert_eq!(snapshot.as_of_ms, 4_501);
    assert_eq!(
        snapshot.blocked_reasons,
        vec![crate::bolt_v3_realized_volatility::RealizedVolBlockReason::QuorumNotReady]
    );
    let diagnostic = snapshot
        .source_diagnostics
        .iter()
        .find(|diagnostic| diagnostic.source_id == TEST_SOURCE_ID)
        .expect("stale source diagnostic should exist");
    assert!(
        matches!(
            diagnostic.block_reason,
            Some(crate::bolt_v3_realized_volatility::RealizedVolBlockReason::SourceStale)
                | Some(crate::bolt_v3_realized_volatility::RealizedVolBlockReason::NotWarm)
        ),
        "expected stale-source diagnostic blocker, got {:?}",
        diagnostic.block_reason
    );
}

#[test]
fn surfaced_realized_volatility_quote_and_trade_sources_can_share_instrument_for_diagnostics() {
    let mut engine_config = test_realized_volatility_engine_config();
    engine_config.sources.push(
        crate::bolt_v3_realized_volatility::RealizedVolSourceConfig {
            source_id: TEST_TRADE_SOURCE_ID.to_string(),
            data_client_id: "<DATA_CLIENT_ID>".to_string(),
            instrument_id: TEST_RV_INSTRUMENT_ID.to_string(),
            source_class: crate::bolt_v3_realized_volatility::RealizedVolSourceClass::Trade,
            sample_kind: crate::bolt_v3_realized_volatility::RealizedVolSampleKind::Trade,
            enabled: true,
            counts_toward_quorum: false,
            canonical_base_asset: "<BASE_ASSET>".to_string(),
            canonical_quote_asset: "<QUOTE_ASSET>".to_string(),
        },
    );
    let mut strategy = test_strategy_with_realized_volatility_surface(engine_config);

    strategy
        .on_quote(&quote_tick(TEST_RV_INSTRUMENT_ID, 100.0, 102.0, 1_000))
        .expect("configured RV quote source should process");
    strategy
        .on_trade(&trade_tick(TEST_RV_INSTRUMENT_ID, 101.0, 1_000))
        .expect("configured RV trade source should process");

    let snapshot = strategy
        .pricing
        .latest_realized_vol_snapshot_for_surface(TEST_SURFACE_ID)
        .expect("RV forwarding should publish a pricing snapshot");
    let quote_diagnostic = snapshot
        .source_diagnostics
        .iter()
        .find(|diagnostic| diagnostic.source_id == TEST_SOURCE_ID)
        .expect("quote source diagnostic should exist");
    let trade_diagnostic = snapshot
        .source_diagnostics
        .iter()
        .find(|diagnostic| diagnostic.source_id == TEST_TRADE_SOURCE_ID)
        .expect("trade source diagnostic should exist");
    assert_eq!(quote_diagnostic.raw_sample_count, 1);
    assert_eq!(trade_diagnostic.raw_sample_count, 1);
    assert_eq!(
        trade_diagnostic.status,
        crate::bolt_v3_realized_volatility::RealizedVolSourceStatus::DiagnosticOnly
    );
}

#[test]
fn interval_open_captures_source_bound_price_to_beat_at_or_after_market_start() {
    let mut strategy = test_strategy();
    let mut snapshot = active_snapshot_with_start("MKT-1", 1_000);
    let SelectionState::Active { market } = &mut snapshot.decision.state else {
        panic!("expected active snapshot");
    };
    market.price_to_beat = Some(3_099.0);
    strategy.apply_selection_snapshot(snapshot);

    strategy.observe_reference_snapshot(&reference_tick(900, 3_100.0));
    assert!(strategy.active.interval_open.is_none());

    strategy.observe_reference_snapshot(&reference_tick(1_000, 3_101.0));
    assert_eq!(strategy.active.interval_open, Some(3_099.0));
}

#[test]
fn interval_open_does_not_use_reference_price_without_source_bound_price_to_beat() {
    let mut strategy = test_strategy();
    strategy.apply_selection_snapshot(active_snapshot_with_start("MKT-1", 1_000));

    strategy.observe_reference_snapshot(&ReferenceSnapshot {
        ts_ms: 1_000,
        topic: "platform.reference.test.spot".to_string(),
        fair_value: Some(3_107.0),
        confidence: 1.0,
        venues: vec![
            oracle_venue("reference", 1.0, 3_100.0, 1_000),
            orderbook_venue("bybit", 0.9, 3_120.0, 1_000),
        ],
    });

    assert_eq!(strategy.active.interval_open, None);
    assert_eq!(strategy.active.last_reference_ts_ms, None);
    assert_eq!(strategy.active.warmup_count, INITIAL_COUNTER_U64);
}

#[test]
fn interval_open_uses_source_bound_price_to_beat_over_reference() {
    let mut strategy = test_strategy();
    let mut snapshot = active_snapshot_with_start("MKT-1", 1_000);
    let SelectionState::Active { market } = &mut snapshot.decision.state else {
        panic!("expected active snapshot");
    };
    market.price_to_beat = Some(3_099.0);
    strategy.apply_selection_snapshot(snapshot);

    strategy.observe_reference_snapshot(&ReferenceSnapshot {
        ts_ms: 1_000,
        topic: "platform.reference.test.spot".to_string(),
        fair_value: Some(3_107.0),
        confidence: 1.0,
        venues: vec![
            oracle_venue("reference", 1.0, 3_100.0, 1_000),
            orderbook_venue("bybit", 0.9, 3_120.0, 1_000),
        ],
    });

    assert_eq!(strategy.active.interval_open, Some(3_099.0));
}

#[test]
fn interval_open_does_not_use_fused_reference_without_source_bound_price_to_beat() {
    let mut strategy = test_strategy();
    strategy.apply_selection_snapshot(active_snapshot_with_start("MKT-1", 1_000));

    strategy.observe_reference_snapshot(&ReferenceSnapshot {
        ts_ms: 1_000,
        topic: "platform.reference.test.spot".to_string(),
        fair_value: Some(3_107.0),
        confidence: 1.0,
        venues: vec![],
    });

    assert_eq!(strategy.active.interval_open, None);
    assert_eq!(strategy.active.last_reference_ts_ms, None);
    assert_eq!(strategy.active.warmup_count, INITIAL_COUNTER_U64);
}

#[test]
fn strike_fetch_reissues_at_interval_open_for_future_next_selection() {
    // B-fetch-at-open regression lock. With `market_selection_rule =
    // "active_or_next"`, the configured target can select a FUTURE "Next"
    // interval whose open boundary is still ahead of wall-clock. The live
    // Chainlink strike report for that interval does not exist yet, so a
    // strike fetch issued before the interval opens cannot bind
    // `price_to_beat`. The strategy must (re)issue the strike fetch once
    // wall-clock reaches the interval open. Today `apply_selection_snapshot`
    // fires `subscribe_resolution_strike` exactly once — when
    // `interval_start_ms` first changes — and never again while the same
    // interval stays selected, so the one-shot fetch fired before open is the
    // only attempt and the strike is permanently stranded. This test drives
    // the production selection path across an interval-open boundary and
    // asserts a SECOND fetch attempt at/after open. It MUST fail until the
    // strategy re-issues the strike fetch at interval open.
    let mut strategy = test_strategy();
    assert_eq!(
        strategy.context.execution_venue(),
        fixture_execution_venue(),
        "harness precondition: production execution venue must be the POLYMARKET fixture",
    );
    // Bind the resolution (strike) instrument to this instance's underlying
    // asset so it clears the fail-closed asset-binding guard inside
    // `subscribe_resolution_strike`; the symbol's leading `-`-segment must
    // equal `underlying_asset`. Without this the live-strike subscribe is a
    // no-op for an unrelated reason (asset mismatch), masking the boundary
    // bug under test.
    strategy.config.resolution_instrument_id = Some(format!(
        "{}-USD.CHAINLINK",
        strategy.config.underlying_asset
    ));
    let cache = register_test_strategy(&mut strategy);

    // Only the FUTURE "Next" interval's instruments exist in the cache. With
    // 300s cadence and a period-aligned base, `next_start` is one full cadence
    // period ahead of the period containing `now_before_open`.
    let cadence_seconds = strategy.config.cadence_seconds as i64;
    let current_period_start = 1_746_000_000_i64;
    let next_period_start = current_period_start + cadence_seconds;
    let next_start_ms = next_period_start as u64 * MILLIS_PER_SECOND_U64;
    let next_end_ms = next_start_ms + strategy.config.cadence_seconds * MILLIS_PER_SECOND_U64;
    let market_slug = crate::bolt_v3_market_families::updown::updown_market_slug(
        &strategy.config.underlying_asset,
        &strategy.config.cadence_slug_token,
        next_period_start,
    );
    let instruments = [
        updown_binary_option(
            "token-up.POLYMARKET",
            &market_slug,
            "market-next",
            "Up",
            next_start_ms,
            next_end_ms,
        ),
        updown_binary_option(
            "token-down.POLYMARKET",
            &market_slug,
            "market-next",
            "Down",
            next_start_ms,
            next_end_ms,
        ),
    ];
    {
        let mut cache_mut = cache.borrow_mut();
        for instrument in &instruments {
            cache_mut
                .add_instrument(instrument.clone())
                .expect("test cache should accept the seeded instrument");
        }
    }

    // 1) Refresh BEFORE the interval opens. `now_before_open` sits in the
    //    period preceding `next_start`, so the configured target selects the
    //    future "Next" interval. The strategy issues its single one-shot
    //    strike fetch here — but the live report for `next_start` does not
    //    exist yet, so this attempt cannot bind the strike.
    let now_before_open_ms = current_period_start as u64 * MILLIS_PER_SECOND_U64 + 1;
    strategy.refresh_selection_from_cache(now_before_open_ms);
    assert_eq!(
        strategy.active.interval_start_ms,
        Some(next_start_ms),
        "precondition: a future Next interval must be selected before its open",
    );
    assert_eq!(
        strategy.active.market_selection_outcome,
        MarketSelectionOutcome::Next,
        "precondition: the selected interval must be the future Next interval",
    );
    assert!(
        strategy.active.price_to_beat.is_none(),
        "precondition: no live strike can exist before the interval opens",
    );
    let fetches_before_open = strategy.resolution_strike_subscribe_count();
    assert_eq!(
        fetches_before_open, 1,
        "the one-shot fetch must fire once when the future interval is first selected",
    );

    // 2) Wall-clock reaches the interval open. The retry-timer path re-runs
    //    selection with `now >= next_start`; the SAME market is now the
    //    Current interval and its live strike report exists. The strategy must
    //    (re)issue the strike fetch for this now-open interval.
    let now_at_open_ms = next_start_ms + 1;
    strategy.refresh_selection_from_cache(now_at_open_ms);
    assert_eq!(
        strategy.active.interval_start_ms,
        Some(next_start_ms),
        "the same interval must still be selected once it opens",
    );

    let fetches_after_open = strategy.resolution_strike_subscribe_count();
    assert!(
        fetches_after_open > fetches_before_open,
        "strike fetch must be re-issued once wall-clock reaches interval open for a future-selected interval that has no strike yet \
         (before-open fetches={fetches_before_open}, after-open fetches={fetches_after_open})",
    );
}

#[test]
fn strike_fetch_retries_each_open_tick_until_price_to_beat_binds() {
    // F4 regression lock. When the at-open strike fetch fails to bind
    // `price_to_beat` (transient REST error, rate-limit, or the report for the
    // exact window-open second has not propagated yet), the strategy must keep
    // re-issuing the fetch on subsequent selection-retry ticks while the
    // interval stays open and `price_to_beat` is still None. The previous
    // implementation marked the interval "subscribed at open" the instant it
    // FIRED the fetch (not when it BOUND), so a single transient failure at the
    // open boundary stranded `price_to_beat = None` for the whole interval and
    // blocked every entry. This drives three retry ticks across the open
    // boundary with no strike ever binding and asserts the fetch is re-issued on
    // each open tick. It MUST fail until the at-open guard tracks binding
    // success rather than fetch-issued.
    let mut strategy = test_strategy();
    strategy.config.resolution_instrument_id = Some(format!(
        "{}-USD.CHAINLINK",
        strategy.config.underlying_asset
    ));
    let cache = register_test_strategy(&mut strategy);

    let cadence_seconds = strategy.config.cadence_seconds as i64;
    let current_period_start = 1_746_000_000_i64;
    let next_period_start = current_period_start + cadence_seconds;
    let next_start_ms = next_period_start as u64 * MILLIS_PER_SECOND_U64;
    let next_end_ms = next_start_ms + strategy.config.cadence_seconds * MILLIS_PER_SECOND_U64;
    let market_slug = crate::bolt_v3_market_families::updown::updown_market_slug(
        &strategy.config.underlying_asset,
        &strategy.config.cadence_slug_token,
        next_period_start,
    );
    let instruments = [
        updown_binary_option(
            "token-up.POLYMARKET",
            &market_slug,
            "market-next",
            "Up",
            next_start_ms,
            next_end_ms,
        ),
        updown_binary_option(
            "token-down.POLYMARKET",
            &market_slug,
            "market-next",
            "Down",
            next_start_ms,
            next_end_ms,
        ),
    ];
    {
        let mut cache_mut = cache.borrow_mut();
        for instrument in &instruments {
            cache_mut
                .add_instrument(instrument.clone())
                .expect("test cache should accept the seeded instrument");
        }
    }

    // Pre-open: the future Next interval is selected; one one-shot fetch fires.
    let now_before_open_ms = current_period_start as u64 * MILLIS_PER_SECOND_U64 + 1;
    strategy.refresh_selection_from_cache(now_before_open_ms);
    assert!(
        strategy.active.price_to_beat.is_none(),
        "precondition: no live strike can exist before the interval opens",
    );
    let fetches_pre_open = strategy.resolution_strike_subscribe_count();

    // First open tick: the at-open fetch fires but never binds (the test
    // subscribe is a no-op that does not deliver an IndexPriceUpdate).
    strategy.refresh_selection_from_cache(next_start_ms + 1);
    let fetches_after_first_open_tick = strategy.resolution_strike_subscribe_count();
    assert!(
        fetches_after_first_open_tick > fetches_pre_open,
        "the strike fetch must be re-issued when the interval first opens",
    );
    assert!(
        strategy.active.price_to_beat.is_none(),
        "precondition: the first at-open fetch did not bind price_to_beat",
    );

    // Second open tick (next retry-timer fire): same interval still open, strike
    // still unbound -> the fetch MUST be re-issued again rather than stranded.
    strategy.refresh_selection_from_cache(next_start_ms + 2);
    let fetches_after_second_open_tick = strategy.resolution_strike_subscribe_count();
    assert!(
        fetches_after_second_open_tick > fetches_after_first_open_tick,
        "strike fetch must keep retrying on each open retry tick while price_to_beat is \
         unbound (after first open tick={fetches_after_first_open_tick}, after \
         second={fetches_after_second_open_tick})",
    );
}

#[test]
fn resolution_strike_reissue_does_not_depend_on_index_unsubscribe_pairing() {
    // Live NT dispatches data commands asynchronously. An immediate
    // index-price unsubscribe/subscribe pair can re-add the exact topic handler
    // before the data engine handles the unsubscribe, causing NT to suppress the
    // provider unsubscribe and dedupe the later subscribe by instrument. Strike
    // re-fetches must therefore use a fetch trigger that does not depend on
    // index-price unsubscribe forwarding.
    let mut strategy = test_strategy();
    strategy.config.resolution_instrument_id = Some(format!(
        "{}-USD.CHAINLINK",
        strategy.config.underlying_asset
    ));
    let cache = register_test_strategy(&mut strategy);

    let cadence_seconds = strategy.config.cadence_seconds as i64;
    let current_period_start = 1_746_000_000_i64;
    let next_period_start = current_period_start + cadence_seconds;
    let next_start_ms = next_period_start as u64 * MILLIS_PER_SECOND_U64;
    let next_end_ms = next_start_ms + strategy.config.cadence_seconds * MILLIS_PER_SECOND_U64;
    let market_slug = crate::bolt_v3_market_families::updown::updown_market_slug(
        &strategy.config.underlying_asset,
        &strategy.config.cadence_slug_token,
        next_period_start,
    );
    let instruments = [
        updown_binary_option(
            "token-up.POLYMARKET",
            &market_slug,
            "market-next",
            "Up",
            next_start_ms,
            next_end_ms,
        ),
        updown_binary_option(
            "token-down.POLYMARKET",
            &market_slug,
            "market-next",
            "Down",
            next_start_ms,
            next_end_ms,
        ),
    ];
    {
        let mut cache_mut = cache.borrow_mut();
        for instrument in &instruments {
            cache_mut
                .add_instrument(instrument.clone())
                .expect("test cache should accept the seeded instrument");
        }
    }

    // Pre-open selection then two open retry ticks: the first attempt should
    // establish the durable index subscription that routes IndexPriceUpdate to
    // on_index_price; later retries should use unique custom fetch commands.
    strategy.refresh_selection_from_cache(current_period_start as u64 * MILLIS_PER_SECOND_U64 + 1);
    strategy.refresh_selection_from_cache(next_start_ms + 1);
    strategy.refresh_selection_from_cache(next_start_ms + 2);

    let events = &strategy.resolution_strike_subscribe_events;
    assert_eq!(
        events.len(),
        3,
        "expected one durable subscribe and two custom retries"
    );
    assert_eq!(
        events[0].trigger,
        ResolutionStrikeFetchTrigger::DurableIndex
    );
    assert!(
        events[0].custom_data_type.is_none(),
        "durable index subscribe must not masquerade as a custom fetch",
    );

    let custom_events = events
        .iter()
        .filter(|event| event.trigger == ResolutionStrikeFetchTrigger::CustomFetch)
        .collect::<Vec<_>>();
    assert_eq!(
        custom_events.len(),
        2,
        "open retry ticks must use the custom fetch trigger path",
    );
    assert_eq!(
        custom_events[0].request_sequence,
        Some(1),
        "first custom retry should carry the first request sequence",
    );
    assert_eq!(
        custom_events[1].request_sequence,
        Some(2),
        "second custom retry should carry the next request sequence",
    );
    let first_topic = custom_events[0]
        .custom_data_type
        .as_ref()
        .expect("custom fetch must record its DataType")
        .topic();
    let second_topic = custom_events[1]
        .custom_data_type
        .as_ref()
        .expect("custom fetch must record its DataType")
        .topic();
    assert_ne!(
        first_topic, second_topic,
        "each retry must have a distinct custom DataType topic so NT dedup cannot swallow it",
    );
}

#[test]
fn strategy_input_evidence_records_source_bound_entry_snapshot_before_order_intent() {
    let evidence = Arc::new(RecordingSequencedDecisionEvidenceWriter::default());
    let submit_admission = submit_admission_with_provider_cap(Decimal::new(1, 2), evidence.clone());
    let mut strategy = ready_to_trade_strategy_with_decision_evidence_and_submit_admission(
        evidence.clone(),
        submit_admission,
    );
    register_test_strategy_with_active_instruments(&mut strategy);

    let error = strategy
        .try_submit_entry_order(1_200)
        .expect_err("submit admission should reject after evidence capture");
    assert!(
        error.to_string().contains("notional cap is exceeded"),
        "{error:#}"
    );

    let events = evidence.events();
    let [
        RecordedDecisionEvidenceEvent::StrategyInput(snapshot),
        RecordedDecisionEvidenceEvent::OrderIntent(intent),
        RecordedDecisionEvidenceEvent::AdmissionDecision(admission),
    ] = events.as_slice()
    else {
        panic!("expected strategy input, order intent, admission sequence; got {events:#?}");
    };

    assert_eq!(snapshot.strategy_id, strategy.config.strategy_id);
    assert_eq!(snapshot.price_to_beat_value, "3100");
    assert_eq!(snapshot.reference_quote_ts_event, 1_200);
    assert_eq!(snapshot.spot_price, "3100.5");
    assert_eq!(snapshot.realized_volatility, "1.5");
    assert_eq!(snapshot.seconds_to_market_end, 300);
    assert_eq!(snapshot.market_id.as_deref(), Some("MKT-1"));
    assert_eq!(
        snapshot.polymarket_condition_id.as_deref(),
        Some("condition-MKT-1")
    );
    assert_eq!(
        snapshot.polymarket_market_slug.as_deref(),
        Some("slug-MKT-1")
    );
    assert_eq!(
        snapshot.polymarket_question_id.as_deref(),
        Some("question-MKT-1")
    );
    assert_eq!(
        snapshot.up_instrument_id.as_deref(),
        Some("condition-MKT-1-MKT-1-UP.POLYMARKET")
    );
    assert_eq!(
        snapshot.down_instrument_id.as_deref(),
        Some("condition-MKT-1-MKT-1-DOWN.POLYMARKET")
    );
    assert_eq!(snapshot.selected_side.as_deref(), Some("up"));
    assert_eq!(snapshot.submission_instrument_id, intent.instrument_id);
    assert_eq!(snapshot.submission_order_side, intent.order_side);
    assert_eq!(snapshot.submission_price, intent.price);
    assert_eq!(snapshot.submission_quantity, intent.quantity);
    assert_eq!(snapshot.client_order_id, intent.client_order_id);
    assert_eq!(admission.client_order_id, intent.client_order_id);
    assert_eq!(
        admission.outcome,
        crate::bolt_v3_decision_evidence::BoltV3AdmissionOutcome::RejectedNotionalCapExceeded
    );
}

#[test]
fn shadow_policy_does_not_leave_pending_entry_between_would_be_entries() {
    let evidence = Arc::new(RecordingSequencedDecisionEvidenceWriter::default());
    let submit_admission = Arc::new(
        crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new(evidence.clone()),
    );
    let mut strategy = ready_to_trade_strategy_with_decision_evidence_and_submit_admission(
        evidence.clone(),
        submit_admission.clone(),
    );
    set_shadow_order_execution_policy(&mut strategy);
    register_test_strategy_with_active_instruments(&mut strategy);

    let first_client_order_id = strategy
        .try_submit_entry_order(1_200)
        .expect("first shadow entry should pass evidence and admission")
        .expect("first shadow entry should produce a would-be client order id");
    assert_eq!(
        strategy.exposure_occupancy(),
        None,
        "shadow entry must not leave a pending exposure without an NT order"
    );

    let second_client_order_id = strategy
        .try_submit_entry_order(1_201)
        .expect("second shadow entry should not be blocked by stale pending exposure")
        .expect("second shadow entry should produce a would-be client order id");
    assert_ne!(first_client_order_id, second_client_order_id);
    assert_eq!(
        submit_admission.admitted_order_count(),
        0,
        "shadow entries must record admission evidence without consuming live submit capacity"
    );
    assert_eq!(
        evidence
            .events()
            .iter()
            .filter(|event| matches!(event, RecordedDecisionEvidenceEvent::OrderIntent(_)))
            .count(),
        2,
        "each shadow entry should still record order-intent evidence"
    );
}

#[test]
fn shadow_policy_entries_do_not_exhaust_live_admission_count_cap() {
    let evidence = Arc::new(RecordingSequencedDecisionEvidenceWriter::default());
    let submit_admission =
        submit_admission_with_provider_cap(Decimal::new(10_000, 0), evidence.clone());
    let mut strategy = ready_to_trade_strategy_with_decision_evidence_and_submit_admission(
        evidence.clone(),
        submit_admission.clone(),
    );
    set_shadow_order_execution_policy(&mut strategy);
    register_test_strategy_with_active_instruments(&mut strategy);

    let first_client_order_id = strategy
        .try_submit_entry_order(1_200)
        .expect("first shadow entry should record observed admission")
        .expect("first shadow entry should produce a would-be client order id");
    let second_client_order_id = strategy
        .try_submit_entry_order(1_201)
        .expect("shadow entry should not consume the live count cap")
        .expect("second shadow entry should produce a would-be client order id");

    assert_ne!(first_client_order_id, second_client_order_id);
    assert_eq!(
        submit_admission.admitted_order_count(),
        0,
        "shadow entries must not consume live submit admission capacity"
    );
    let admission_outcomes = evidence
        .events()
        .into_iter()
        .filter_map(|event| match event {
            RecordedDecisionEvidenceEvent::AdmissionDecision(admission) => Some(admission.outcome),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        admission_outcomes,
        vec![
            crate::bolt_v3_decision_evidence::BoltV3AdmissionOutcome::Admitted,
            crate::bolt_v3_decision_evidence::BoltV3AdmissionOutcome::Admitted,
        ],
        "shadow mode should still record admitted decisions for each would-be entry"
    );
}

#[test]
fn shadow_policy_exit_keeps_pending_exit_between_would_be_exits() {
    let evidence = Arc::new(RecordingSequencedDecisionEvidenceWriter::default());
    let submit_admission = Arc::new(
        crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new(evidence.clone()),
    );
    let mut strategy = ready_to_trade_strategy_with_decision_evidence_and_submit_admission(
        evidence.clone(),
        submit_admission.clone(),
    );
    set_shadow_order_execution_policy(&mut strategy);
    strategy.active.phase = SelectionPhase::Freeze;
    register_test_strategy_with_active_instruments(&mut strategy);
    let instrument_id = configured_outcome_instruments(&strategy)
        .into_iter()
        .next()
        .expect("ready-to-trade fixture should expose an outcome instrument");
    let position_quantity = Quantity::new(strategy.config.order_notional_target, 2);
    let position = materialize_configured_position(
        &mut strategy,
        instrument_id,
        PositionId::from("P-SHADOW-EXIT-001"),
        position_quantity,
        0.45,
    );
    set_managed_position(
        &mut strategy,
        position,
        ManagedPositionOrigin::StrategyEntry,
    );

    let first_client_order_id = strategy
        .try_submit_exit_order(
            1_200,
            crate::bolt_v3_decision_evidence::BoltV3ExitTriggerSource::SelectionUpdate,
            Some(1_200),
            None,
        )
        .expect("first shadow exit should pass evidence and admission")
        .expect("first shadow exit should produce a would-be client order id");
    assert_eq!(
        strategy.exposure_occupancy(),
        Some(ExposureOccupancy::ExitPending),
        "shadow exit must keep the live-mode pending-exit latch"
    );
    assert_eq!(
        pending_exit_ref(&strategy).map(|pending| pending.client_order_id),
        Some(first_client_order_id)
    );

    assert_eq!(
        strategy
            .try_submit_exit_order(
                1_201,
                crate::bolt_v3_decision_evidence::BoltV3ExitTriggerSource::SelectionUpdate,
                Some(1_201),
                None,
            )
            .expect("latched shadow exit should not fail"),
        None,
        "latched shadow exit should block repeated would-be exits"
    );
    assert_eq!(
        submit_admission.admitted_order_count(),
        0,
        "shadow exits must record admission evidence without consuming live submit capacity"
    );
    assert_eq!(
        evidence
            .events()
            .iter()
            .filter(|event| matches!(event, RecordedDecisionEvidenceEvent::OrderIntent(_)))
            .count(),
        1,
        "latched shadow exit should record one order-intent"
    );
}

#[test]
fn shadow_policy_surfaces_admission_rejection_and_clears_pending_entry() {
    let evidence = Arc::new(RecordingSequencedDecisionEvidenceWriter::default());
    let submit_admission = submit_admission_with_provider_cap(Decimal::new(1, 2), evidence.clone());
    let mut strategy = ready_to_trade_strategy_with_decision_evidence_and_submit_admission(
        evidence.clone(),
        submit_admission.clone(),
    );
    set_shadow_order_execution_policy(&mut strategy);
    register_test_strategy_with_active_instruments(&mut strategy);

    let error = strategy.try_submit_entry_order(1_200).expect_err(
        "a shadow admission rejection must still surface as Err via the non-consuming path",
    );
    assert!(
        error.to_string().contains("notional cap is exceeded"),
        "{error:#}"
    );
    assert_eq!(
        strategy.exposure_occupancy(),
        None,
        "a rejected shadow entry must clear pending-entry exposure, not latch a phantom order"
    );
    assert_eq!(
        submit_admission.admitted_order_count(),
        0,
        "a rejected shadow entry must not consume live submit capacity"
    );
    let admission_outcomes = evidence
        .events()
        .into_iter()
        .filter_map(|event| match event {
            RecordedDecisionEvidenceEvent::AdmissionDecision(admission) => Some(admission.outcome),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        admission_outcomes,
        vec![crate::bolt_v3_decision_evidence::BoltV3AdmissionOutcome::RejectedNotionalCapExceeded],
        "a rejected shadow entry must still record the rejected admission decision"
    );
}

#[test]
fn strategy_input_evidence_records_realized_volatility_unknown_source_rejections() {
    let evidence = Arc::new(RecordingSequencedDecisionEvidenceWriter::default());
    let submit_admission = submit_admission_with_provider_cap(Decimal::new(1, 2), evidence.clone());
    let mut strategy = ready_to_trade_strategy_with_decision_evidence_and_submit_admission(
        evidence.clone(),
        submit_admission,
    );
    register_test_strategy_with_active_instruments(&mut strategy);
    strategy.config.realized_volatility_surface_id = TEST_SURFACE_ID.to_string();
    strategy
        .pricing
        .set_realized_volatility_surface_id(TEST_SURFACE_ID.to_string());
    let mut unknown_source_rejections = std::collections::BTreeMap::new();
    unknown_source_rejections.insert("<UNKNOWN_SOURCE_ID>".to_string(), 2);
    strategy.pricing.observe_realized_vol_snapshot(
        crate::bolt_v3_realized_volatility::RealizedVolSnapshot {
            surface_id: TEST_SURFACE_ID.to_string(),
            as_of_ms: 1_200,
            annualized_realized_vol_decimal: Some(1.5),
            measured_annualized_realized_vol_decimal: Some(1.5),
            noise_robust_annualized_realized_vol_decimal: Some(1.5),
            continuous_annualized_realized_vol_decimal: Some(1.5),
            jump_annualized_realized_vol_decimal: Some(0.0),
            forecast_annualized_realized_vol_decimal: None,
            pricing_component:
                crate::bolt_v3_realized_volatility::RealizedVolPricingComponent::Measured,
            ready: true,
            sources_used: vec![TEST_SOURCE_ID.to_string()],
            source_diagnostics: Vec::new(),
            horizon_estimates: Vec::new(),
            unknown_source_rejections,
            blocked_reasons: Vec::new(),
            aggregate_method:
                crate::bolt_v3_realized_volatility::RealizedVolAggregation::UpperQuantile {
                    quantile: 1.0,
                },
            seconds_per_annum: 31_536_000.0,
            config_fingerprint: "<config_fingerprint>".to_string(),
        },
    );

    strategy
        .try_submit_entry_order(1_200)
        .expect_err("submit admission should reject after evidence capture");

    let events = evidence.events();
    let Some(RecordedDecisionEvidenceEvent::StrategyInput(snapshot)) = events.first() else {
        panic!("expected first evidence event to be strategy input; got {events:#?}");
    };
    assert_eq!(
        snapshot
            .realized_volatility_unknown_source_rejections
            .get("<UNKNOWN_SOURCE_ID>"),
        Some(&2)
    );
}

#[test]
fn strategy_input_evidence_accepts_ready_surfaced_zero_realized_volatility() {
    let evidence = Arc::new(RecordingSequencedDecisionEvidenceWriter::default());
    let submit_admission = submit_admission_with_provider_cap(Decimal::new(1, 2), evidence.clone());
    let mut strategy = ready_to_trade_strategy_with_decision_evidence_and_submit_admission(
        evidence.clone(),
        submit_admission,
    );
    register_test_strategy_with_active_instruments(&mut strategy);
    strategy.config.realized_volatility_surface_id = TEST_SURFACE_ID.to_string();
    strategy
        .pricing
        .set_realized_volatility_surface_id(TEST_SURFACE_ID.to_string());
    strategy.pricing.observe_realized_vol_snapshot(
        crate::bolt_v3_realized_volatility::RealizedVolSnapshot {
            surface_id: TEST_SURFACE_ID.to_string(),
            as_of_ms: 1_200,
            annualized_realized_vol_decimal: Some(0.0),
            measured_annualized_realized_vol_decimal: Some(0.0),
            noise_robust_annualized_realized_vol_decimal: Some(0.0),
            continuous_annualized_realized_vol_decimal: Some(0.0),
            jump_annualized_realized_vol_decimal: Some(0.0),
            forecast_annualized_realized_vol_decimal: None,
            pricing_component:
                crate::bolt_v3_realized_volatility::RealizedVolPricingComponent::Measured,
            ready: true,
            sources_used: vec![TEST_SOURCE_ID.to_string()],
            source_diagnostics: Vec::new(),
            horizon_estimates: Vec::new(),
            unknown_source_rejections: std::collections::BTreeMap::new(),
            blocked_reasons: Vec::new(),
            aggregate_method:
                crate::bolt_v3_realized_volatility::RealizedVolAggregation::UpperQuantile {
                    quantile: 1.0,
                },
            seconds_per_annum: 31_536_000.0,
            config_fingerprint: "<config_fingerprint>".to_string(),
        },
    );

    let error = strategy
        .try_submit_entry_order(1_200)
        .expect_err("submit admission should reject after evidence capture");
    assert!(
        error.to_string().contains("notional cap is exceeded"),
        "{error:#}"
    );

    let events = evidence.events();
    let Some(RecordedDecisionEvidenceEvent::StrategyInput(snapshot)) = events.first() else {
        panic!("expected first evidence event to be strategy input; got {events:#?}");
    };
    assert_eq!(snapshot.realized_volatility, "0");
    assert_eq!(snapshot.realized_volatility_annualized_decimal, "0");
}

#[test]
fn strategy_input_evidence_records_realized_volatility_not_ready_pricing_block() {
    let evidence = Arc::new(RecordingSequencedDecisionEvidenceWriter::default());
    let submit_admission = submit_admission_with_provider_cap(Decimal::new(1, 2), evidence.clone());
    let mut strategy = ready_to_trade_strategy_with_decision_evidence_and_submit_admission(
        evidence.clone(),
        submit_admission,
    );
    register_test_strategy_with_active_instruments(&mut strategy);
    strategy.config.realized_volatility_surface_id = TEST_SURFACE_ID.to_string();
    strategy
        .pricing
        .set_realized_volatility_surface_id(TEST_SURFACE_ID.to_string());
    strategy.pricing.observe_realized_vol_snapshot(
        crate::bolt_v3_realized_volatility::RealizedVolSnapshot {
            surface_id: TEST_SURFACE_ID.to_string(),
            as_of_ms: 1_200,
            annualized_realized_vol_decimal: None,
            measured_annualized_realized_vol_decimal: None,
            noise_robust_annualized_realized_vol_decimal: None,
            continuous_annualized_realized_vol_decimal: None,
            jump_annualized_realized_vol_decimal: None,
            forecast_annualized_realized_vol_decimal: None,
            pricing_component:
                crate::bolt_v3_realized_volatility::RealizedVolPricingComponent::Measured,
            ready: false,
            sources_used: Vec::new(),
            source_diagnostics: Vec::new(),
            horizon_estimates: Vec::new(),
            unknown_source_rejections: std::collections::BTreeMap::new(),
            blocked_reasons: vec![
                crate::bolt_v3_realized_volatility::RealizedVolBlockReason::QuorumNotReady,
            ],
            aggregate_method:
                crate::bolt_v3_realized_volatility::RealizedVolAggregation::UpperQuantile {
                    quantile: 1.0,
                },
            seconds_per_annum: 31_536_000.0,
            config_fingerprint: "<config_fingerprint>".to_string(),
        },
    );

    assert_eq!(
        strategy
            .try_submit_entry_order(1_200)
            .expect("RV-not-ready pricing block should not attempt submit"),
        None
    );

    let events = evidence.events();
    let [RecordedDecisionEvidenceEvent::StrategyInput(snapshot)] = events.as_slice() else {
        panic!("expected only blocked strategy input evidence; got {events:#?}");
    };
    assert_eq!(snapshot.realized_volatility_surface_id, TEST_SURFACE_ID);
    assert_eq!(snapshot.realized_volatility_as_of_ms, Some(1_200));
    assert_eq!(snapshot.realized_volatility, "");
    assert_eq!(snapshot.realized_volatility_annualized_decimal, "");
    assert_eq!(
        snapshot.realized_volatility_blockers,
        vec!["quorum_not_ready".to_string()]
    );
    assert_eq!(snapshot.submission_instrument_id, "");
    assert_eq!(snapshot.client_order_id, "");
}

#[test]
fn strategy_input_evidence_market_end_uses_selection_expiry_not_remaining_seconds() {
    let evidence = Arc::new(RecordingSequencedDecisionEvidenceWriter::default());
    let submit_admission = submit_admission_with_provider_cap(Decimal::new(1, 2), evidence.clone());
    let mut strategy = ready_to_trade_strategy_with_decision_evidence_and_submit_admission(
        evidence.clone(),
        submit_admission,
    );
    register_test_strategy_with_active_instruments(&mut strategy);
    strategy.active.interval_end_ms = Some(301_999);

    strategy
        .try_submit_entry_order(2_000)
        .expect_err("submit admission should reject after evidence capture");

    let events = evidence.events();
    let Some(RecordedDecisionEvidenceEvent::StrategyInput(snapshot)) = events.first() else {
        panic!("expected first evidence event to be strategy input; got {events:#?}");
    };

    assert_eq!(snapshot.seconds_to_market_end, 299);
    assert_eq!(snapshot.market_selection_timestamp_ms, Some(1_000));
    assert_eq!(snapshot.polymarket_market_start_timestamp_ms, Some(1_000));
    assert_eq!(
        snapshot.polymarket_market_end_timestamp_ms,
        Some(301_999),
        "market end must bind to selected expiration without seconds rounding"
    );
}

#[test]
fn strategy_input_evidence_records_next_market_selection_outcome() {
    let evidence = Arc::new(RecordingSequencedDecisionEvidenceWriter::default());
    let submit_admission = submit_admission_with_provider_cap(Decimal::new(1, 2), evidence.clone());
    let mut strategy = ready_to_trade_strategy_with_decision_evidence_and_submit_admission(
        evidence.clone(),
        submit_admission,
    );
    register_test_strategy_with_active_instruments(&mut strategy);
    strategy.active.market_selection_outcome = MarketSelectionOutcome::Next;

    strategy
        .try_submit_entry_order(2_000)
        .expect_err("submit admission should reject after evidence capture");

    let events = evidence.events();
    let Some(RecordedDecisionEvidenceEvent::StrategyInput(snapshot)) = events.first() else {
        panic!("expected first evidence event to be strategy input; got {events:#?}");
    };

    assert_eq!(snapshot.market_selection_outcome, "next");
}

#[test]
fn observe_resolution_strike_binds_strike_at_interval_open() {
    let mut active =
        ActiveMarketState::from_snapshot(&active_snapshot_with_start("MKT-1", 1_000), 0);
    assert_eq!(active.phase, SelectionPhase::Active);
    assert_eq!(active.interval_start_ms, Some(1_000));
    assert_eq!(active.price_to_beat, None);

    active.observe_resolution_strike(3_100.5, 1_000, 1_250);

    assert_eq!(
        active.price_to_beat,
        Some(3_100.5),
        "a positive strike bound to the interval-open must set price_to_beat"
    );
    assert_eq!(active.last_resolution_ts_ms, Some(1_250));
}

#[test]
fn observe_resolution_strike_rejects_mismatched_window_fail_closed() {
    let mut active =
        ActiveMarketState::from_snapshot(&active_snapshot_with_start("MKT-1", 1_000), 0);
    assert_eq!(active.interval_start_ms, Some(1_000));

    // Window-open boundary does not match the market's interval-open.
    active.observe_resolution_strike(3_100.5, 2_000, 2_250);

    assert_eq!(
        active.price_to_beat, None,
        "a strike whose window-open does not match the interval-open must be ignored (fail-closed)"
    );
    assert_eq!(active.last_resolution_ts_ms, None);
}

/// H-observe-log: a window-mismatch rejection in a *configured* (non-Idle,
/// interval-bound) market is an actionable fail-closed anomaly — the strike
/// feed disagrees with the selected interval. It must be observable, and it
/// must be observably distinct from an Idle drop (where nothing is running,
/// so a mismatched update is simply not relevant).
///
/// Behavioral contract under both cases: `price_to_beat` stays `None`. The
/// distinguishing observable: a non-Idle mismatch records an observable
/// rejection, while an Idle drop does not.
#[test]
fn observe_resolution_strike_window_mismatch_is_observable_and_distinct_from_idle() {
    // Configured (non-Idle) market whose interval-open is 1_000.
    let mut active =
        ActiveMarketState::from_snapshot(&active_snapshot_with_start("MKT-1", 1_000), 0);
    assert_eq!(active.phase, SelectionPhase::Active);
    assert_eq!(active.interval_start_ms, Some(1_000));
    assert_eq!(active.price_to_beat, None);
    assert_eq!(active.resolution_strike_window_mismatch_count, 0);

    // A strike whose window-open (2_000) does not match the interval-open.
    active.observe_resolution_strike(3_100.5, 2_000, 2_250);

    // Behavioral contract: fail-closed, price_to_beat untouched.
    assert_eq!(
        active.price_to_beat, None,
        "configured window mismatch must leave price_to_beat None (fail-closed)"
    );
    assert_eq!(active.last_resolution_ts_ms, None);

    // Observable rejection: a configured mismatch must be recorded so the
    // anomaly is visible, not silently dropped.
    assert_eq!(
        active.resolution_strike_window_mismatch_count, 1,
        "a configured (non-Idle) window mismatch must record an observable rejection"
    );

    // An Idle drop is NOT the same event: nothing is configured, so a
    // mismatched update is irrelevant and must not be recorded as an anomaly.
    let mut idle = ActiveMarketState::idle();
    assert_eq!(idle.phase, SelectionPhase::Idle);
    assert_eq!(idle.resolution_strike_window_mismatch_count, 0);

    idle.observe_resolution_strike(3_100.5, 2_000, 2_250);

    assert_eq!(
        idle.price_to_beat, None,
        "Idle drop must leave price_to_beat None"
    );
    assert_eq!(
        idle.resolution_strike_window_mismatch_count, 0,
        "an Idle drop must be handled distinctly from a configured mismatch \
         (no observable rejection recorded)"
    );
}

/// Build a ready-to-trade strategy with one open managed position and a recording
/// evidence writer, for #885 exit-evaluation evidence tests. Returns the strategy
/// and the writer (the caller drives exits and reads back `events()`).
fn exit_evidence_strategy_with_open_position() -> (
    BinaryOracleEdgeTaker,
    Arc<RecordingSequencedDecisionEvidenceWriter>,
) {
    let evidence = Arc::new(RecordingSequencedDecisionEvidenceWriter::default());
    let strategy = exit_evidence_strategy_with_open_position_using_writer(evidence.clone());
    (strategy, evidence)
}

/// Build the open-position exit-evidence fixture against an arbitrary decision
/// evidence writer. Shared by the recording-writer tests and the failing-writer
/// swallow test so the open-position setup lives in ONE place.
fn exit_evidence_strategy_with_open_position_using_writer(
    evidence: Arc<dyn crate::bolt_v3_decision_evidence::BoltV3DecisionEvidenceWriter>,
) -> BinaryOracleEdgeTaker {
    let submit_admission = Arc::new(
        crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new(evidence.clone()),
    );
    let mut strategy = ready_to_trade_strategy_with_decision_evidence_and_submit_admission(
        evidence,
        submit_admission,
    );
    // Shadow policy so a would-be exit submits through the admission/evidence path
    // without consuming live submit capacity.
    set_shadow_order_execution_policy(&mut strategy);
    strategy.active.phase = SelectionPhase::Freeze;
    register_test_strategy_with_active_instruments(&mut strategy);
    let instrument_id = configured_outcome_instruments(&strategy)
        .into_iter()
        .next()
        .expect("ready-to-trade fixture should expose an outcome instrument");
    let position_quantity = Quantity::new(strategy.config.order_notional_target, 2);
    let position = materialize_configured_position(
        &mut strategy,
        instrument_id,
        PositionId::from("P-EXIT-EVIDENCE-001"),
        position_quantity,
        0.45,
    );
    set_managed_position(
        &mut strategy,
        position,
        ManagedPositionOrigin::StrategyEntry,
    );
    strategy
}

/// Collect every recorded exit-evaluation evidence record, in order.
fn recorded_exit_evaluations(
    evidence: &RecordingSequencedDecisionEvidenceWriter,
) -> Vec<crate::bolt_v3_decision_evidence::BoltV3ExitEvaluationEvidence> {
    evidence
        .events()
        .into_iter()
        .filter_map(|event| match event {
            RecordedDecisionEvidenceEvent::ExitEvaluation(evidence) => Some(*evidence),
            _ => None,
        })
        .collect()
}

#[test]
fn exit_evaluation_evidence_write_failure_does_not_change_exit_submission() {
    // FIX 3b: the exit-evaluation evidence sink is swallow-on-error. A writer that
    // errors only on record_exit_evaluation must leave the exit submission result
    // identical to a recording writer, with no panic.
    let (mut control_strategy, _control_evidence) = exit_evidence_strategy_with_open_position();
    control_strategy
        .pricing
        .seed_ready_realized_vol(Some("<SOURCE_ID>".to_string()), 1.5, 1_200);
    let control_result = control_strategy
        .try_submit_exit_order(
            1_200,
            crate::bolt_v3_decision_evidence::BoltV3ExitTriggerSource::SignalQuote,
            Some(1_200),
            Some(1_180),
        )
        .expect("control exit evaluation should not error with a ready realized-vol surface");

    let failing_evidence = Arc::new(ExitEvaluationFailingDecisionEvidenceWriter::default());
    let mut failing_strategy =
        exit_evidence_strategy_with_open_position_using_writer(failing_evidence.clone());
    failing_strategy
        .pricing
        .seed_ready_realized_vol(Some("<SOURCE_ID>".to_string()), 1.5, 1_200);
    let failing_result = failing_strategy
        .try_submit_exit_order(
            1_200,
            crate::bolt_v3_decision_evidence::BoltV3ExitTriggerSource::SignalQuote,
            Some(1_200),
            Some(1_180),
        )
        .expect("a failing exit-evaluation sink must be swallowed, not propagated");

    // The trading-side result is structurally identical with and without the sink
    // failure (the client order id itself is a fresh UUID per run, so compare the
    // submit/no-submit shape, not the minted id).
    assert_eq!(control_result.is_some(), failing_result.is_some());
    // The swallow path was exercised: the sink was reached and did error.
    assert_eq!(
        failing_evidence.exit_evaluation_attempts(),
        1,
        "the exit-evaluation sink must have been attempted exactly once"
    );
}

#[test]
fn exit_evaluation_evidence_records_accepted_rv_gate() {
    let (mut strategy, evidence) = exit_evidence_strategy_with_open_position();
    // RV ready at as_of == now == 1_200 → the gate accepts the snapshot.
    strategy
        .pricing
        .seed_ready_realized_vol(Some("<SOURCE_ID>".to_string()), 1.5, 1_200);

    strategy
        .try_submit_exit_order(
            1_200,
            crate::bolt_v3_decision_evidence::BoltV3ExitTriggerSource::SignalQuote,
            Some(1_200),
            Some(1_180),
        )
        .expect("exit evaluation should not error with a ready realized-vol surface");

    let records = recorded_exit_evaluations(&evidence);
    assert_eq!(
        records.len(),
        1,
        "an accepted exit evaluation must record exactly one durable evidence record"
    );
    let record = &records[0];
    assert_eq!(
        record.rv_gate_result,
        crate::bolt_v3_decision_evidence::BoltV3RvGateResult::Accepted,
        "a fresh, ready realized-vol snapshot must classify as Accepted"
    );
    assert_eq!(record.exit_eval_now_ms, 1_200);
    assert_eq!(record.rv_as_of_ms, Some(1_200));
    assert!(
        record.rv_ready,
        "an accepted snapshot must report rv_ready=true"
    );
    assert_eq!(
        record.exit_trigger_source,
        crate::bolt_v3_decision_evidence::BoltV3ExitTriggerSource::SignalQuote,
        "the durable record must preserve the triggering runtime path"
    );
    assert_eq!(record.trigger_ts_event_ms, Some(1_200));
    assert_eq!(record.trigger_ts_init_ms, Some(1_180));
}

#[test]
fn exit_evaluation_evidence_records_future_dated_rv_gate_with_delta() {
    let (mut strategy, evidence) = exit_evidence_strategy_with_open_position();
    // Re-seed the realized-vol snapshot dated 800ms in the FUTURE relative to the
    // exit-evaluation clock (the 2026-06-20 incident's root cause shape): as_of 2_000
    // while the exit is evaluated at now 1_200.
    strategy
        .pricing
        .seed_ready_realized_vol(Some("<SOURCE_ID>".to_string()), 1.5, 2_000);

    strategy
        .try_submit_exit_order(
            1_200,
            crate::bolt_v3_decision_evidence::BoltV3ExitTriggerSource::BookDelta,
            Some(1_190),
            None,
        )
        .expect("exit evaluation should not error with a future-dated realized-vol surface");

    let records = recorded_exit_evaluations(&evidence);
    assert_eq!(
        records.len(),
        1,
        "a future-dated exit evaluation must record exactly one durable evidence record"
    );
    let record = &records[0];
    assert_eq!(
        record.rv_gate_result,
        crate::bolt_v3_decision_evidence::BoltV3RvGateResult::RejectedFutureDated,
        "a snapshot dated after now must classify as RejectedFutureDated"
    );
    assert_eq!(record.exit_eval_now_ms, 1_200);
    assert_eq!(record.rv_as_of_ms, Some(2_000));
    assert_eq!(
        record.rv_as_of_minus_now_ms,
        Some(800),
        "the durable record must capture the as_of-minus-now delta for RCA"
    );
}

#[test]
fn exit_evaluation_evidence_flood_guard_collapses_repeated_outcomes() {
    let (mut strategy, evidence) = exit_evidence_strategy_with_open_position();
    strategy
        .pricing
        .seed_ready_realized_vol(Some("<SOURCE_ID>".to_string()), 1.5, 1_200);

    // First evaluation submits a would-be exit (one record, outcome key = Exit).
    strategy
        .try_submit_exit_order(
            1_200,
            crate::bolt_v3_decision_evidence::BoltV3ExitTriggerSource::SignalQuote,
            Some(1_200),
            None,
        )
        .expect("first shadow exit should pass evidence and admission");

    // The position is now latched as ExitPending. Drive four MORE evaluations: every
    // one produces the identical latched outcome key (Hold / exit_already_pending /
    // Accepted). The first latched tick is a key change (one record); the remaining
    // three are identical and MUST be suppressed by the flood guard.
    for tick in 1_201..=1_204 {
        assert_eq!(
            strategy
                .try_submit_exit_order(
                    tick,
                    crate::bolt_v3_decision_evidence::BoltV3ExitTriggerSource::SignalQuote,
                    Some(tick as i64),
                    None,
                )
                .expect("latched exit evaluation should not error"),
            None,
            "a latched exit must not submit a repeated would-be order"
        );
    }

    let records = recorded_exit_evaluations(&evidence);
    assert_eq!(
        records.len(),
        2,
        "the flood guard must collapse five identical-outcome exit ticks into one \
         submit record plus one latched-transition record (without the guard this \
         would be five records)"
    );
}
