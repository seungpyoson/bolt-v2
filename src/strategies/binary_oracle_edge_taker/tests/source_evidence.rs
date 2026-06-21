#![cfg(test)]

use super::*;

const TEST_SURFACE_ID: &str = "<surface_id>";
const TEST_SOURCE_ID: &str = "<SOURCE_ID_A>";
const TEST_SOURCE_ID_B: &str = "<SOURCE_ID_B>";
const TEST_TRADE_SOURCE_ID: &str = "<SOURCE_ID_TRADE>";
const TEST_RV_INSTRUMENT_ID: &str = "<INSTRUMENT_ID_A>.<DATA_CLIENT_ID>";

#[derive(Default)]
struct CapturingLogger {
    records: std::sync::Mutex<Vec<(log::Level, String)>>,
}

impl log::Log for CapturingLogger {
    fn enabled(&self, _metadata: &log::Metadata<'_>) -> bool {
        true
    }

    fn log(&self, record: &log::Record<'_>) {
        self.records
            .lock()
            .expect("capturing logger mutex poisoned")
            .push((record.level(), record.args().to_string()));
    }

    fn flush(&self) {}
}

impl CapturingLogger {
    fn reset(&self) {
        self.records
            .lock()
            .expect("capturing logger mutex poisoned")
            .clear();
    }

    fn records(&self) -> Vec<(log::Level, String)> {
        self.records
            .lock()
            .expect("capturing logger mutex poisoned")
            .clone()
    }
}

static CAPTURING_LOGGER: std::sync::OnceLock<&'static CapturingLogger> = std::sync::OnceLock::new();
static CAPTURING_LOGGER_OBSERVERS: std::sync::Mutex<()> = std::sync::Mutex::new(());
static NEXT_LOG_CAPTURE_STRATEGY_ID: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);
const LOG_CAPTURE_CHILD_ENV: &str = "BOLT_TAKER_SOURCE_EVIDENCE_LOG_CAPTURE";

fn unique_log_capture_strategy_id(prefix: &str) -> String {
    let id = NEXT_LOG_CAPTURE_STRATEGY_ID.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    format!("BINARYORACLEEDGETAKER-{prefix}-{id}")
}

fn install_capturing_logger() -> &'static CapturingLogger {
    static INSTALL_OUTCOME: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    let logger = CAPTURING_LOGGER.get_or_init(|| Box::leak(Box::new(CapturingLogger::default())));
    let installed = *INSTALL_OUTCOME.get_or_init(|| log::set_logger(*logger).is_ok());
    assert!(
        installed,
        "capturing logger could not claim the global log slot; another logger is installed"
    );
    log::set_max_level(log::LevelFilter::Trace);
    *logger
}

fn run_log_capture_test_in_subprocess(test_filter: &str, mode: &str) {
    let output = std::process::Command::new(
        std::env::current_exe().expect("current test binary should be available"),
    )
    .arg(test_filter)
    .arg("--nocapture")
    .arg("--test-threads=1")
    .env(LOG_CAPTURE_CHILD_ENV, mode)
    .output()
    .expect("log-capture child test should launch");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "log-capture child test failed with status {:?}\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        stdout,
        stderr
    );
    assert!(
        stdout.contains("running 1 test"),
        "log-capture child filter `{test_filter}` must run exactly one test\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
}

fn with_captured_error_log<R>(
    failure_message: &str,
    strategy_id: &str,
    action: impl FnOnce() -> R,
) -> R {
    let logger = install_capturing_logger();
    let _observer_guard = CAPTURING_LOGGER_OBSERVERS
        .lock()
        .expect("capturing logger observer mutex poisoned");
    logger.reset();

    let result = action();

    let matching = logger
        .records()
        .into_iter()
        .filter(|(_, message)| message.contains(failure_message) && message.contains(strategy_id))
        .collect::<Vec<_>>();
    assert_eq!(
        matching.len(),
        1,
        "{failure_message} must be surfaced exactly once for {strategy_id}; got {matching:?}"
    );
    assert_eq!(
        matching[0].0,
        log::Level::Error,
        "{failure_message} must be surfaced at error! severity, not warn!"
    );
    result
}

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
    strategy.pricing.last_fast_venue_age_ms = Some(17);
    strategy.pricing.last_fast_venue_jitter_ms = Some(3);
    strategy.pricing.last_lead_agreement_corr = Some(0.99);
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
    assert!(
        snapshot
            .up_worst_case_edge_basis_points
            .as_deref()
            .and_then(|value| value.parse::<f64>().ok())
            .is_some(),
        "admitted entry snapshot must preserve the up-side thin margin"
    );
    assert!(
        snapshot
            .down_worst_case_edge_basis_points
            .as_deref()
            .and_then(|value| value.parse::<f64>().ok())
            .is_some(),
        "admitted entry snapshot must preserve the down-side thin margin"
    );
    assert!(snapshot.gate_blocked_by.is_empty());
    assert!(snapshot.pricing_blocked_by.is_empty());
    assert_eq!(snapshot.fast_venue_name.as_deref(), Some("bybit"));
    assert_eq!(snapshot.fast_venue_age_ms, Some(17));
    assert_eq!(snapshot.fast_venue_jitter_ms, Some(3));
    assert!(!snapshot.fast_venue_incoherent);
    assert_eq!(
        snapshot
            .lead_agreement_corr
            .as_deref()
            .and_then(|value| value.parse::<f64>().ok()),
        Some(0.99)
    );
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
        .try_submit_exit_order(1_200)
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
            .try_submit_exit_order(1_201)
            .expect("latched shadow exit should not fail"),
        None,
        "latched shadow exit should block repeated would-be exits"
    );
    assert_eq!(
        strategy
            .try_submit_exit_order(1_202)
            .expect("same latched shadow exit should remain deduped"),
        None,
        "same latched shadow exit state should not flood decision evidence"
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
    let exit_decisions = evidence
        .events()
        .into_iter()
        .filter_map(|event| match event {
            RecordedDecisionEvidenceEvent::ExitDecision(decision) => Some(decision),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        exit_decisions.len(),
        2,
        "exit action plus one pending-exit block should be recorded once each"
    );
    assert_eq!(
        exit_decisions[0].exit_decision,
        BoltV3ExitDecisionOutcome::Exit
    );
    assert_eq!(
        exit_decisions[0].forced_flat_reasons,
        vec![BoltV3ForcedFlatReason::Freeze]
    );
    assert_eq!(exit_decisions[0].exit_eval_now_ms, 1_200);
    assert_eq!(
        exit_decisions[0].exit_trigger_source,
        BoltV3ExitTriggerSource::Unknown
    );
    assert_eq!(exit_decisions[0].trigger_ts_event_ms, 1_200);
    assert_eq!(exit_decisions[0].trigger_ts_init_ms, None);
    assert_eq!(exit_decisions[0].rv_surface_id, TEST_SURFACE_ID);
    assert_eq!(exit_decisions[0].rv_snapshot_as_of_ms, Some(1_200));
    assert!(exit_decisions[0].rv_snapshot_ready);
    assert_eq!(exit_decisions[0].rv_snapshot_blockers, Vec::new());
    assert_eq!(
        exit_decisions[0].rv_gate_result,
        BoltV3ExitRvGateResult::Accepted
    );
    assert_eq!(exit_decisions[0].rv_future_dating_delta_ms, None);
    assert_eq!(
        exit_decisions[1].exit_decision,
        BoltV3ExitDecisionOutcome::Blocked
    );
    assert_eq!(
        exit_decisions[1].blocked_reason,
        Some(BoltV3ExitBlockedReason::ExitAlreadyPending)
    );
}

#[test]
fn signal_quote_exit_decision_records_future_dated_realized_volatility_gate() {
    let evidence = Arc::new(RecordingSequencedDecisionEvidenceWriter::default());
    let submit_admission = Arc::new(
        crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new(evidence.clone()),
    );
    let mut strategy = ready_to_trade_strategy_with_decision_evidence_and_submit_admission(
        evidence.clone(),
        submit_admission,
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
        PositionId::from(stringify!(P_SHADOW_EXIT_FUTURE_RV)),
        position_quantity,
        strategy
            .active
            .books
            .up
            .best_ask
            .expect("ready-to-trade fixture should expose an up ask"),
    );
    set_managed_position(
        &mut strategy,
        position,
        ManagedPositionOrigin::StrategyEntry,
    );

    let exit_eval_now_ms = strategy
        .active
        .last_reference_ts_ms
        .expect("ready-to-trade fixture should carry a reference timestamp");
    let future_delta_ms = strategy.config.market_exit_interval_ms;
    let future_as_of_ms = exit_eval_now_ms + future_delta_ms;
    let realized_vol = strategy
        .current_realized_vol_at(exit_eval_now_ms)
        .expect("fixture should start with accepted realized volatility");
    strategy.pricing.observe_realized_vol_snapshot(
        crate::bolt_v3_realized_volatility::RealizedVolSnapshot {
            surface_id: strategy.config.realized_volatility_surface_id.clone(),
            as_of_ms: future_as_of_ms,
            annualized_realized_vol_decimal: Some(realized_vol),
            measured_annualized_realized_vol_decimal: Some(realized_vol),
            noise_robust_annualized_realized_vol_decimal: Some(realized_vol),
            continuous_annualized_realized_vol_decimal: Some(realized_vol),
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
            seconds_per_annum: test_realized_volatility_engine_config().seconds_per_annum,
            config_fingerprint: String::new(),
        },
    );
    assert_eq!(strategy.current_realized_vol_at(exit_eval_now_ms), None);

    let signal_instrument_id = strategy
        .config
        .signal_instrument_id
        .as_deref()
        .expect("ready-to-trade fixture should configure a signal instrument")
        .to_string();
    let signal_bid = strategy
        .active
        .price_to_beat
        .expect("ready-to-trade fixture should carry source-bound price_to_beat");
    let signal_ask = strategy
        .pricing
        .last_reference_current_price()
        .expect("ready-to-trade fixture should carry a reference current price");
    strategy
        .on_quote(&quote_tick(
            &signal_instrument_id,
            signal_bid,
            signal_ask,
            exit_eval_now_ms,
        ))
        .expect("signal quote trigger should process");

    let exit_decisions = evidence
        .events()
        .into_iter()
        .filter_map(|event| match event {
            RecordedDecisionEvidenceEvent::ExitDecision(decision) => Some(decision),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(exit_decisions.len(), 1);
    let decision = &exit_decisions[0];
    assert_eq!(decision.realized_vol, None);
    assert_eq!(decision.rv_snapshot_as_of_ms, Some(future_as_of_ms));
    assert!(decision.rv_snapshot_ready);
    assert_eq!(
        decision.rv_gate_result,
        BoltV3ExitRvGateResult::RejectedFutureDated
    );
    assert_eq!(decision.rv_future_dating_delta_ms, Some(future_delta_ms));
    assert_eq!(
        decision.exit_trigger_source,
        BoltV3ExitTriggerSource::SignalQuote
    );
    assert_eq!(decision.trigger_ts_event_ms, exit_eval_now_ms);
    assert_eq!(decision.trigger_ts_init_ms, Some(exit_eval_now_ms));
    assert_eq!(decision.exit_eval_now_ms, exit_eval_now_ms);
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
    assert_eq!(
        strategy
            .try_submit_entry_order(1_201)
            .expect("same RV-not-ready pricing block should not attempt submit"),
        None
    );

    let events = evidence.events();
    let Some(RecordedDecisionEvidenceEvent::StrategyInput(snapshot)) = events.first() else {
        panic!("expected blocked strategy input evidence first; got {events:#?}");
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
    assert_eq!(
        snapshot.pricing_blocked_by,
        vec![BoltV3EntryPricingBlockReason::RealizedVolNotReady]
    );
    let entry_skips = events
        .iter()
        .filter_map(|event| match event {
            RecordedDecisionEvidenceEvent::EntrySkip(skip) => Some(skip),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        entry_skips.len(),
        1,
        "same blocked interval/reason must emit one entry skip record"
    );
    let skip = entry_skips[0];
    assert_eq!(
        skip.reason_category,
        BoltV3EntrySkipReasonCategory::EntryPricingBlocked
    );
    assert_eq!(
        skip.pricing_blocked_by,
        vec![BoltV3EntryPricingBlockReason::RealizedVolNotReady]
    );
    assert_eq!(skip.market_id, strategy.active.market_id);
    assert_eq!(skip.realized_vol_source_ts_ms, Some(1_200));
}

#[test]
fn entry_skip_evidence_records_distinct_pricing_blockers_in_same_interval() {
    let evidence = Arc::new(RecordingSequencedDecisionEvidenceWriter::default());
    let submit_admission = submit_admission_with_provider_cap(Decimal::new(1, 2), evidence.clone());
    let mut strategy = ready_to_trade_strategy_with_decision_evidence_and_submit_admission(
        evidence.clone(),
        submit_admission,
    );
    register_test_strategy_with_active_instruments(&mut strategy);

    let mut realized_vol_not_ready = minimal_entry_submission_decision();
    realized_vol_not_ready.evaluation.pricing_blocked_by =
        vec![EntryPricingBlockReason::RealizedVolNotReady];
    let mut fee_unavailable = minimal_entry_submission_decision();
    fee_unavailable.evaluation.pricing_blocked_by =
        vec![EntryPricingBlockReason::FeeUnavailable(OutcomeSide::Up)];

    strategy
        .record_entry_skip_once(
            1_200,
            &realized_vol_not_ready,
            BoltV3EntrySkipReasonCategory::EntryPricingBlocked,
            None,
        )
        .expect("first pricing-blocked skip should record");
    strategy
        .record_entry_skip_once(
            1_201,
            &fee_unavailable,
            BoltV3EntrySkipReasonCategory::EntryPricingBlocked,
            None,
        )
        .expect("distinct pricing blocker in same interval should record");

    let entry_skips = evidence
        .events()
        .into_iter()
        .filter_map(|event| match event {
            RecordedDecisionEvidenceEvent::EntrySkip(skip) => Some(skip),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        entry_skips.len(),
        2,
        "same interval/category but different pricing blockers must not dedupe"
    );
    assert_eq!(entry_skips[0].market_id, strategy.active.market_id);
    assert_eq!(entry_skips[1].market_id, strategy.active.market_id);
    assert_eq!(entry_skips[0].market_id, entry_skips[1].market_id);
    assert_eq!(
        entry_skips[0].pricing_blocked_by,
        vec![BoltV3EntryPricingBlockReason::RealizedVolNotReady]
    );
    assert_eq!(
        entry_skips[1].pricing_blocked_by,
        vec![BoltV3EntryPricingBlockReason::FeeUnavailable(
            BoltV3OutcomeSide::Up
        )]
    );
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

#[test]
fn parse_config_rejects_non_finite_forced_flat_thin_book_min_liquidity() {
    for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let mut raw = valid_raw_config();
        raw.as_table_mut()
            .expect("raw config should be a TOML table")
            .insert(
                stringify!(forced_flat_thin_book_min_liquidity).to_string(),
                toml::Value::Float(bad),
            );

        let err = BinaryOracleEdgeTakerBuilder::parse_config(&raw)
            .expect_err("non-finite forced-flat liquidity minimum must be rejected");

        assert!(
            err.to_string()
                .contains("forced_flat_thin_book_min_liquidity must be finite and >= 0"),
            "expected forced-flat liquidity finite rejection for {bad}, got: {err}"
        );
    }
}

#[test]
fn forced_flat_evidence_filters_non_finite_liquidity_values() {
    let mut strategy = ready_to_trade_strategy();
    register_test_strategy_with_active_instruments(&mut strategy);
    strategy.config.forced_flat_thin_book_min_liquidity = f64::NAN;
    strategy.active.books.up.liquidity_available = Some(f64::INFINITY);
    strategy.active.books.down.liquidity_available = Some(f64::INFINITY);

    let entry_inputs = strategy.entry_forced_flat_evidence_inputs();
    assert_eq!(
        entry_inputs.min_liquidity_required, None,
        "non-finite configured minimum must not serialize into forced-flat entry evidence"
    );
    assert_eq!(
        entry_inputs.liquidity_available, None,
        "non-finite active liquidity must not serialize into forced-flat entry evidence"
    );

    let exit_inputs = strategy.exit_forced_flat_evidence_inputs();
    assert_eq!(
        exit_inputs.min_liquidity_required, None,
        "non-finite configured minimum must not serialize into forced-flat exit evidence"
    );
    assert_eq!(
        exit_inputs.liquidity_available, None,
        "non-finite active liquidity must not serialize into forced-flat exit evidence"
    );
}

// A minimal entry evaluation that carries no signal — enough to drive
// `record_entry_skip_once` past evidence assembly to the writer call.
fn minimal_entry_evaluation() -> EntryEvaluation {
    EntryEvaluation {
        gate: EntryGateDecision { blocked_by: vec![] },
        pricing_blocked_by: vec![],
        fair_probability_up: None,
        uncertainty_band_probability: None,
        up_executable_edge: None,
        down_executable_edge: None,
        up_worst_case_ev_bps: None,
        down_worst_case_ev_bps: None,
        sized_executable_edge: None,
        sized_worst_case_ev_bps: None,
        min_worst_case_ev_bps: None,
        expected_ev_per_notional: None,
        book_impact_cap_notional: None,
        sized_notional: None,
        selected_side: None,
    }
}

fn minimal_entry_submission_decision() -> EntrySubmissionDecision {
    EntrySubmissionDecision {
        evaluation: minimal_entry_evaluation(),
        instrument_id: None,
        order_side: None,
        price: None,
        quantity_value: None,
        client_order_id: None,
        blocked_reason: None,
    }
}

// A minimal exit decision with a non-`no_open_position` block reason, so it
// clears the early-return guards in `record_exit_decision_once` and reaches the
// writer call.
fn minimal_exit_submission_decision() -> ExitSubmissionDecision {
    ExitSubmissionDecision {
        evaluation: ExitEvaluation {
            position_outcome_side: None,
            forced_flat_reasons: vec![],
            hold_ev_bps: None,
            exit_ev_bps: None,
            exit_decision: Some(ExitDecision::Hold),
            blocked_reason: Some(EXIT_BLOCK_REASON_EXIT_HOLD),
        },
        instrument_id: None,
        order_type: None,
        order_side: None,
        position_side: None,
        time_in_force: None,
        price: None,
        quantity: None,
        client_order_id: None,
        is_post_only: None,
        is_reduce_only: None,
        is_quote_quantity: None,
        expire_time_unix_nanos: None,
        trigger_price: None,
        activation_price: None,
        trigger_type: None,
        trigger_instrument_id: None,
        trailing_offset: None,
        trailing_offset_type: None,
        blocked_reason: Some(EXIT_BLOCK_REASON_EXIT_HOLD),
        forced_flat_reasons: vec![],
    }
}

#[test]
fn exit_decision_evidence_write_failure_does_not_block_the_exit() {
    // A telemetry-write failure on the exit-decision evidence path MUST NOT
    // propagate: record_exit_decision_once is called at the exit-submit chokepoint
    // immediately BEFORE the risk-reducing exit order is built and submitted. The
    // pre-fix `record_exit_decision(&evidence)?` propagated the writer Err, which
    // would abort the exit-submit callback and BLOCK a risk-reducing exit on a lost
    // log line. The fix swaps `?` for a `log::error!` + continue, so the helper
    // returns Ok(()) even when the writer fails. The FailingDecisionEvidenceWriter
    // returns Err from record_exit_decision, so on the buggy `?` variant this call
    // returns Err and the assertion below fails — the differential channel is the
    // helper's Result, which mirrors the exit-submit return.
    if std::env::var(LOG_CAPTURE_CHILD_ENV).ok().as_deref() != Some("exit") {
        run_log_capture_test_in_subprocess(
            "exit_decision_evidence_write_failure_does_not_block_the_exit",
            "exit",
        );
        return;
    }

    let mut strategy = test_strategy_with_fee_provider_and_decision_evidence(
        RecordingFeeProvider::cold(),
        Arc::new(FailingDecisionEvidenceWriter),
    );
    let strategy_id = unique_log_capture_strategy_id("exit");
    strategy.config.strategy_id = strategy_id.clone();

    let decision = minimal_exit_submission_decision();
    let result = with_captured_error_log(
        "binary_oracle_edge_taker exit decision evidence write failed",
        &strategy_id,
        || {
            strategy.record_exit_decision_once(
                1_000,
                ExitEvaluationTriggerContext::unknown(1_000),
                &decision,
            )
        },
    );

    assert!(
        result.is_ok(),
        "an exit-decision evidence write failure must not propagate and block the exit: {result:?}"
    );
    assert!(
        strategy.last_recorded_exit_decision.is_some(),
        "the dedupe key must still be set so the lost write is not retried in a tight loop"
    );
}

#[test]
fn entry_skip_evidence_write_failure_does_not_abort_the_strategy_callback() {
    // An entry skip is DECLINING new risk. record_entry_skip_once is called inside
    // the entry-submit callback immediately before downstream safety logic (e.g.
    // enforce_one_position_invariant). The pre-fix `record_entry_skip(&evidence)?`
    // propagated the writer Err, aborting the callback and SKIPPING that downstream
    // safety logic on a lost log line. The fix swaps `?` for a `log::error!` +
    // continue, so the helper returns Ok(()) even when the writer fails. The
    // FailingDecisionEvidenceWriter returns Err from record_entry_skip, so on the
    // buggy `?` variant this call returns Err and the assertion fails — the
    // differential channel is the helper's Result.
    if std::env::var(LOG_CAPTURE_CHILD_ENV).ok().as_deref() != Some("entry") {
        run_log_capture_test_in_subprocess(
            "entry_skip_evidence_write_failure_does_not_abort_the_strategy_callback",
            "entry",
        );
        return;
    }

    let mut strategy = test_strategy_with_fee_provider_and_decision_evidence(
        RecordingFeeProvider::cold(),
        Arc::new(FailingDecisionEvidenceWriter),
    );
    let strategy_id = unique_log_capture_strategy_id("entry");
    strategy.config.strategy_id = strategy_id.clone();

    let decision = minimal_entry_submission_decision();
    let result = with_captured_error_log(
        "binary_oracle_edge_taker entry skip evidence write failed",
        &strategy_id,
        || {
            strategy.record_entry_skip_once(
                1_000,
                &decision,
                BoltV3EntrySkipReasonCategory::NoSideSelected,
                None,
            )
        },
    );

    assert!(
        result.is_ok(),
        "an entry-skip evidence write failure must not abort the strategy callback: {result:?}"
    );
    assert!(
        strategy.last_recorded_entry_skip.is_some(),
        "the dedupe key must still be set so the lost write is not retried in a tight loop"
    );
}
