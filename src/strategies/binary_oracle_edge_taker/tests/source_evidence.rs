#![cfg(test)]

use super::*;
use nautilus_common::messages::data::{DataCommand, SubscribeCommand};

const TEST_SURFACE_ID: &str = "<surface_id>";
const TEST_SOURCE_ID: &str = "<SOURCE_ID_A>";
const TEST_SOURCE_ID_B: &str = "<SOURCE_ID_B>";
const TEST_TRADE_SOURCE_ID: &str = "<SOURCE_ID_TRADE>";
const TEST_RV_INSTRUMENT_ID: &str = "<INSTRUMENT_ID_A>.<DATA_CLIENT_ID>";
const STRATEGY_INPUT_QUOTE_REPLAY: &str =
    include_str!("../../../../tests/fixtures/bolt_v3/strategy_input_quote_replay.toml");

#[derive(Debug, serde::Deserialize)]
struct StrategyInputQuoteReplay {
    market_start_ms: u64,
    evaluation_now_ms: u64,
    forced_flat_stale_reference_ms: u64,
    reference_max_source_age_ms: u64,
    realized_vol: f64,
    reference: ReplayReferenceQuote,
    signal: ReplaySignalQuote,
}

#[derive(Debug, serde::Deserialize)]
struct ReplayReferenceQuote {
    source_id: String,
    provider: String,
    provider_instrument: String,
    price: f64,
    observed_ts_ms: u64,
    received_ts_ms: u64,
}

#[derive(Debug, serde::Deserialize)]
struct ReplaySignalQuote {
    instrument_id: String,
    bid: f64,
    ask: f64,
    observed_ts_ms: u64,
}

fn strategy_input_quote_replay() -> StrategyInputQuoteReplay {
    toml::from_str(STRATEGY_INPUT_QUOTE_REPLAY)
        .expect("strategy input quote replay fixture should parse")
}

fn replay_reference_price_config(
    replay: &StrategyInputQuoteReplay,
) -> crate::bolt_v3_config::ReferencePriceBlock {
    crate::bolt_v3_config::ReferencePriceBlock {
        asset: "BTC".to_string(),
        source_order: vec![replay.reference.source_id.clone()],
        min_valid_sources: 1,
        selection_policy:
            crate::bolt_v3_config::ReferencePriceSelectionPolicy::FirstValidPerInterval,
        max_source_age_ms: replay.reference_max_source_age_ms,
        max_source_drift_bps: 25,
        drift_policy: crate::bolt_v3_config::ReferencePriceDriftPolicy::Observe,
        stale_policy: crate::bolt_v3_config::ReferencePriceStalePolicy::Block,
        sources: std::collections::BTreeMap::from([(
            replay.reference.source_id.clone(),
            crate::bolt_v3_config::ReferencePriceSourceBlock {
                provider: crate::bolt_v3_config::ReferencePriceProvider::new(
                    replay.reference.provider.clone(),
                )
                .expect("fixture provider should be valid"),
                enabled: true,
                required: false,
                client_id: nautilus_model::identifiers::ClientId::from("chainlink_reference"),
                instrument_id: Some(replay.reference.provider_instrument.clone()),
                symbol: None,
            },
        )]),
    }
}

fn replay_reference_update(replay: &StrategyInputQuoteReplay) -> nautilus_model::data::CustomData {
    replay_reference_update_at(
        replay,
        replay.reference.price,
        replay.reference.observed_ts_ms,
        replay.reference.received_ts_ms,
    )
}

fn replay_reference_update_at(
    replay: &StrategyInputQuoteReplay,
    price: f64,
    observed_ts_ms: u64,
    received_ts_ms: u64,
) -> nautilus_model::data::CustomData {
    crate::bolt_v3_reference_price::ReferencePriceUpdate::try_new(
        "BTC",
        replay.reference.source_id.as_str(),
        replay.reference.provider.as_str(),
        replay.reference.provider_instrument.as_str(),
        price,
        None,
        None,
        observed_ts_ms,
        received_ts_ms,
    )
    .expect("replay reference quote should construct")
    .to_custom_data()
}

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

fn with_captured_strategy_logs<R>(
    strategy_id: &str,
    action: impl FnOnce() -> R,
) -> (R, Vec<(log::Level, String)>) {
    let logger = install_capturing_logger();
    let _observer_guard = CAPTURING_LOGGER_OBSERVERS
        .lock()
        .expect("capturing logger observer mutex poisoned");
    logger.reset();

    let result = action();
    let matching = logger
        .records()
        .into_iter()
        .filter(|(_, message)| message.contains(strategy_id))
        .collect::<Vec<_>>();
    (result, matching)
}

fn test_realized_volatility_engine_config()
-> crate::bolt_v3_realized_volatility::RealizedVolEngineConfig {
    crate::bolt_v3_realized_volatility::RealizedVolEngineConfig {
        surface_id: TEST_SURFACE_ID.to_string(),
        window_ms: 4_000,
        sampling_interval_ms: 1_000,
        min_ready_sources: 1,
        max_source_age_ms: 500,
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
fn strategy_on_start_dispatches_real_data_subscribe_commands() {
    let replay = strategy_input_quote_replay();
    let mut strategy =
        test_strategy_with_realized_volatility_surface(test_realized_volatility_engine_config());
    strategy.config.reference_current_price = Some(replay_reference_price_config(&replay));
    register_test_strategy(&mut strategy);

    DataActor::on_start(&mut strategy).expect("strategy should start");

    let commands = recorded_data_commands();
    assert!(
        commands.iter().any(|command| matches!(
            command,
            DataCommand::Subscribe(SubscribeCommand::Quotes(command))
                if command.instrument_id
                    == strategy
                        .signal_instrument_id()
                        .expect("signal instrument should parse")
                    && command.client_id == strategy.signal_client_id()
        )),
        "on_start must enqueue the configured signal quote subscription through NT DataActor; commands={commands:#?}",
    );
    assert!(
        commands.iter().any(|command| matches!(
            command,
            DataCommand::Subscribe(SubscribeCommand::Data(command))
                if command.client_id
                    == Some(nautilus_model::identifiers::ClientId::from("chainlink_reference"))
                    && command.data_type.type_name() == "BoltV3ReferencePriceUpdate"
        )),
        "on_start must enqueue configured reference-current-price custom-data subscriptions through NT DataActor; commands={commands:#?}",
    );
    assert!(
        commands.iter().any(|command| matches!(
            command,
            DataCommand::Subscribe(SubscribeCommand::Quotes(command))
                if command.instrument_id
                    == nautilus_model::identifiers::InstrumentId::from(TEST_RV_INSTRUMENT_ID)
                    && command.client_id
                        == Some(nautilus_model::identifiers::ClientId::from("<DATA_CLIENT_ID>"))
        )),
        "on_start must enqueue configured realized-volatility source subscriptions through NT DataActor; commands={commands:#?}",
    );
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
fn surfaced_realized_volatility_refresh_preserves_event_domain_as_of() {
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

    strategy.refresh_realized_volatility_snapshot_at(4_500);

    assert!(strategy.current_realized_vol_at(4_500).is_some());
    let snapshot = strategy
        .pricing
        .latest_realized_vol_snapshot_for_surface(TEST_SURFACE_ID)
        .expect("RV refresh should publish a pricing snapshot");
    assert_eq!(snapshot.as_of_ms, 4_000);
    assert!(snapshot.ready);
    assert!(snapshot.blocked_reasons.is_empty());
    let diagnostic = snapshot
        .source_diagnostics
        .iter()
        .find(|diagnostic| diagnostic.source_id == TEST_SOURCE_ID)
        .expect("stale source diagnostic should exist");
    assert_eq!(diagnostic.last_sample_ts_ms, Some(4_000));
    assert_eq!(diagnostic.block_reason, None);
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
    let strategy_id = unique_log_capture_strategy_id("entry-admitted");
    strategy.config.strategy_id = strategy_id.clone();
    strategy.pricing.last_fast_venue_age_ms = Some(17);
    strategy.pricing.last_fast_venue_jitter_ms = Some(3);
    strategy.pricing.last_lead_agreement_corr = Some(probability(0.99));
    register_test_strategy_with_active_instruments(&mut strategy);
    strategy.observe_reference_snapshot(&ReferenceSnapshot {
        ts_ms: 1_200,
        topic: "platform.reference.test.spot".to_string(),
        fair_value: Some(3_100.5),
        confidence: 1.0,
        venues: vec![
            oracle_venue("reference", 1.0, 3_100.5, 1_200),
            orderbook_venue("bybit", 0.9, 3_100.5, 1_200),
        ],
    });
    strategy.pricing.last_fast_venue_age_ms = Some(17);
    strategy.pricing.last_fast_venue_jitter_ms = Some(3);
    strategy.pricing.last_lead_agreement_corr = Some(probability(0.99));

    let (submit_result, logs) =
        with_captured_strategy_logs(&strategy_id, || strategy.try_submit_entry_order(1_200));
    let error = submit_result.expect_err("submit admission should reject after evidence capture");
    assert!(
        error.to_string().contains("notional cap is exceeded"),
        "{error:#}"
    );
    let entry_evaluation_logs = logs
        .iter()
        .filter(|(_, message)| {
            message.contains("binary_oracle_edge_taker entry evaluation:")
                && message.contains(&strategy_id)
        })
        .collect::<Vec<_>>();
    assert_eq!(
        entry_evaluation_logs.len(),
        1,
        "admitted entry should emit one entry-evaluation log for {strategy_id}: {logs:?}"
    );
    let entry_evaluation_log = &entry_evaluation_logs[0].1;
    assert!(
        entry_evaluation_log.contains("fast_venue_available=true"),
        "entry-evaluation log must expose admitted spot state: {entry_evaluation_log}"
    );
    assert!(
        entry_evaluation_log.contains("reference_current_price_available=true"),
        "entry-evaluation log must expose standalone admitted reference state: {entry_evaluation_log}"
    );
    assert!(
        entry_evaluation_log.contains("reference_current_price_available_without_fast_venue=false"),
        "entry-evaluation log must keep the conjunction marker separate: {entry_evaluation_log}"
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
    assert!(
        snapshot.fast_venue_available,
        "admitted entry snapshot must expose admitted spot state"
    );
    assert!(
        snapshot.reference_current_price_available,
        "admitted entry snapshot must expose admitted reference state"
    );
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
fn rv_clock_domain_amendment_entry_log_uses_admitted_receipt_after_snapshot_replacement() {
    let (mut strategy, _) = admitted_entry_strategy_for_rv_receipt();
    strategy
        .pricing
        .seed_ready_realized_vol(Some(TEST_SOURCE_ID.to_string()), 1.5, 1_200);

    let decision = strategy.entry_submission_decision_at(1_200);
    assert_eq!(
        decision.blocked_reason, None,
        "fixture must admit the entry"
    );

    replace_rv_with_distinguishable_snapshot(&mut strategy);

    let fields = strategy.entry_evaluation_log_fields_at(1_300, &decision);
    assert_eq!(fields.realized_vol, Some(1.5));
    assert_eq!(
        fields.realized_vol_source_venue.as_deref(),
        Some(TEST_SOURCE_ID)
    );
    assert_eq!(fields.realized_vol_source_ts_ms, Some(1_200));
    assert_eq!(
        fields.realized_vol_gate_result,
        BoltV3RvGateResult::Accepted
    );
    assert_eq!(
        fields.realized_vol_receive_watermark_ms,
        Some(LocalReceiveMs::new(1_200))
    );
}

#[test]
fn rv_clock_domain_amendment_entry_skip_uses_admitted_receipt_after_snapshot_replacement() {
    let (mut strategy, evidence) = admitted_entry_strategy_for_rv_receipt();
    strategy
        .pricing
        .seed_ready_realized_vol(Some(TEST_SOURCE_ID.to_string()), 1.5, 1_200);

    let decision = strategy.entry_submission_decision_at(1_200);
    assert_eq!(
        decision.blocked_reason, None,
        "fixture must admit the entry"
    );

    replace_rv_with_distinguishable_snapshot(&mut strategy);
    strategy
        .record_entry_skip_once(
            1_300,
            &decision,
            BoltV3EntrySkipReasonCategory::NoSideSelected,
            None,
        )
        .expect("skip evidence should record from the admitted receipt");

    let events = evidence.events();
    let Some(RecordedDecisionEvidenceEvent::EntrySkip(skip)) = events.first() else {
        panic!("expected entry-skip evidence; got {events:#?}");
    };
    assert_eq!(skip.realized_vol.as_deref(), Some("1.5"));
    assert_eq!(
        skip.realized_vol_source_venue.as_deref(),
        Some(TEST_SOURCE_ID)
    );
    assert_eq!(skip.realized_vol_source_ts_ms, Some(1_200));
    assert_eq!(skip.realized_vol_gate_result, BoltV3RvGateResult::Accepted);
    assert_eq!(
        skip.realized_vol_receive_watermark_ms,
        Some(LocalReceiveMs::new(1_200))
    );
}

#[test]
fn rv_clock_domain_amendment_submit_evidence_cannot_veto_admitted_order() {
    let (mut strategy, evidence) = admitted_entry_strategy_for_rv_receipt();
    strategy
        .pricing
        .seed_ready_realized_vol(Some(TEST_SOURCE_ID.to_string()), 1.5, 1_200);

    let decision = strategy.entry_submission_decision_at(1_200);
    assert_eq!(
        decision.blocked_reason, None,
        "fixture must admit the entry"
    );
    replace_rv_with_distinguishable_snapshot(&mut strategy);

    let submitted = strategy
        .submit_admitted_entry_decision(9_000, decision)
        .expect("later wall time and replacement must not veto admitted submit");
    assert!(
        submitted.is_some(),
        "the admitted order must reach submit routing"
    );
    let snapshot = evidence
        .events()
        .into_iter()
        .find_map(|event| match event {
            RecordedDecisionEvidenceEvent::StrategyInput(snapshot) => Some(snapshot),
            _ => None,
        })
        .expect("actual submit route must persist strategy-input evidence");
    assert_eq!(snapshot.realized_volatility, "1.5");
    assert_eq!(snapshot.realized_volatility_surface_id, TEST_SURFACE_ID);
    assert_eq!(snapshot.realized_volatility_as_of_ms, Some(1_200));
    assert_eq!(
        snapshot.realized_volatility_sources_used,
        vec![TEST_SOURCE_ID.to_string()]
    );
    assert_eq!(
        snapshot.realized_volatility_gate_result,
        BoltV3RvGateResult::Accepted
    );
    assert_eq!(
        snapshot.realized_volatility_receive_watermark_ms,
        Some(LocalReceiveMs::new(1_200))
    );
    assert_eq!(snapshot.realized_volatility_annualized_decimal, "1.5");
    assert_eq!(
        snapshot.realized_volatility_measured_annualized_decimal,
        "1.5"
    );
    assert_eq!(
        snapshot.realized_volatility_noise_robust_annualized_decimal,
        "1.5"
    );
    assert_eq!(
        snapshot.realized_volatility_continuous_annualized_decimal,
        "1.5"
    );
    assert_eq!(snapshot.realized_volatility_jump_annualized_decimal, "0");
    assert_eq!(snapshot.realized_volatility_forecast_annualized_decimal, "");
    assert_eq!(snapshot.realized_volatility_pricing_component, "measured");
    assert_eq!(snapshot.realized_volatility_seconds_per_annum, "31536000");
    assert_eq!(snapshot.realized_volatility_aggregation, "upper_quantile");
    assert!(snapshot.realized_volatility_source_diagnostics.is_empty());
    assert!(
        snapshot
            .realized_volatility_unknown_source_rejections
            .is_empty()
    );
    assert!(snapshot.realized_volatility_blockers.is_empty());
    assert!(snapshot.realized_volatility_config_fingerprint.is_empty());
}

fn replace_rv_with_distinguishable_snapshot(strategy: &mut BinaryOracleEdgeTaker) {
    let mut replacement = strategy
        .pricing
        .latest_realized_vol_snapshot_for_surface(TEST_SURFACE_ID)
        .expect("fixture must contain admitted snapshot")
        .clone();
    replacement.as_of_ms = 1_300;
    replacement.latest_accepted_receive_ms = Some(LocalReceiveMs::new(1_301));
    replacement.annualized_realized_vol_decimal = Some(9.5);
    replacement.measured_annualized_realized_vol_decimal = Some(9.4);
    replacement.noise_robust_annualized_realized_vol_decimal = Some(9.3);
    replacement.continuous_annualized_realized_vol_decimal = Some(9.2);
    replacement.jump_annualized_realized_vol_decimal = Some(9.1);
    replacement.forecast_annualized_realized_vol_decimal = Some(9.0);
    replacement.pricing_component =
        crate::bolt_v3_realized_volatility::RealizedVolPricingComponent::Forecast;
    replacement.seconds_per_annum = 31_535_999.0;
    replacement.aggregate_method =
        crate::bolt_v3_realized_volatility::RealizedVolAggregation::TrimmedMean {
            trim_fraction: 0.1,
        };
    replacement.sources_used = vec![TEST_SOURCE_ID_B.to_string()];
    replacement
        .unknown_source_rejections
        .insert(TEST_SOURCE_ID_B.to_string(), 7);
    replacement.blocked_reasons =
        vec![crate::bolt_v3_realized_volatility::RealizedVolBlockReason::QuorumNotReady];
    replacement.config_fingerprint = "replacement-fingerprint".to_string();
    strategy.pricing.observe_realized_vol_snapshot(replacement);
}

fn admitted_entry_strategy_for_rv_receipt() -> (
    BinaryOracleEdgeTaker,
    Arc<RecordingSequencedDecisionEvidenceWriter>,
) {
    let evidence = Arc::new(RecordingSequencedDecisionEvidenceWriter::default());
    let submit_admission = Arc::new(
        crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new(evidence.clone()),
    );
    let mut strategy = ready_to_trade_strategy_with_decision_evidence_and_submit_admission(
        evidence.clone(),
        submit_admission,
    );
    register_test_strategy_with_active_instruments(&mut strategy);
    (strategy, evidence)
}

#[test]
fn blocked_entry_replay_records_observed_spot_and_reference_inputs() {
    let replay = strategy_input_quote_replay();
    let evidence = Arc::new(RecordingSequencedDecisionEvidenceWriter::default());
    let submit_admission = Arc::new(
        crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new(evidence.clone()),
    );
    let mut strategy = ready_to_trade_strategy_with_decision_evidence_and_submit_admission(
        evidence.clone(),
        submit_admission,
    );
    let strategy_id = unique_log_capture_strategy_id("entry-fallback");
    strategy.config.strategy_id = strategy_id.clone();
    strategy.config.reference_current_price = Some(replay_reference_price_config(&replay));
    strategy.config.forced_flat_stale_reference_ms = replay.forced_flat_stale_reference_ms;
    strategy.apply_selection_snapshot(active_snapshot_with_start("MKT-1", replay.market_start_ms));
    strategy.active.price_to_beat = Some(replay.reference.price);
    strategy.active.interval_open = Some(replay.reference.price);
    strategy.active.warmup_count = strategy.config.warmup_tick_count;
    strategy.active.outcome_fees.up_ready = true;
    strategy.active.outcome_fees.down_ready = true;
    strategy.active.books.up.last_observed_instrument_id = strategy.active.books.up.instrument_id;
    strategy
        .active
        .books
        .up
        .bid_levels
        .insert(Price::new(0.50, 2), 5_000.0);
    strategy
        .active
        .books
        .up
        .ask_levels
        .insert(Price::new(0.50, 2), 5_000.0);
    strategy.active.books.up.best_bid = Some(0.50);
    strategy.active.books.up.best_ask = Some(0.50);
    strategy.active.books.up.liquidity_available = Some(5_000.0);
    strategy.active.books.down.last_observed_instrument_id =
        strategy.active.books.down.instrument_id;
    strategy
        .active
        .books
        .down
        .bid_levels
        .insert(Price::new(0.48, 2), 5_000.0);
    strategy
        .active
        .books
        .down
        .ask_levels
        .insert(Price::new(0.49, 2), 5_000.0);
    strategy.active.books.down.best_bid = Some(0.48);
    strategy.active.books.down.best_ask = Some(0.49);
    strategy.active.books.down.liquidity_available = Some(5_000.0);
    strategy.pricing.set_selected_pricing_spot(None);
    strategy.pricing.seed_ready_realized_vol(
        Some("fixture_replay".to_string()),
        replay.realized_vol,
        replay.reference.observed_ts_ms,
    );

    let (cache, clock) = register_test_strategy_with_clock(&mut strategy);
    add_active_instruments_to_cache(&strategy, &cache);
    clock.borrow_mut().set_time(UnixNanos::from(
        replay
            .reference
            .received_ts_ms
            .saturating_mul(NANOS_PER_MILLI_U64),
    ));

    let reference_update = replay_reference_update(&replay);
    DataActor::on_data(&mut strategy, &reference_update)
        .expect("replay reference quote should be observed");
    assert_eq!(
        strategy.active.reference_current_price,
        Some(replay.reference.price)
    );

    clock.borrow_mut().set_time(UnixNanos::from(
        replay.evaluation_now_ms.saturating_mul(NANOS_PER_MILLI_U64),
    ));
    strategy.refresh_current_reference_price_selection_at(replay.evaluation_now_ms);
    assert_eq!(
        strategy.active.last_reference_ts_ms,
        Some(replay.reference.observed_ts_ms)
    );
    assert_eq!(strategy.active.reference_current_price, None);

    let signal_quote = quote_tick(
        replay.signal.instrument_id.as_str(),
        replay.signal.bid,
        replay.signal.ask,
        replay.signal.observed_ts_ms,
    );
    DataActor::on_quote(&mut strategy, &signal_quote)
        .expect("replay signal quote should be observed");
    assert_eq!(strategy.pricing.spot_price(), None);
    assert!(strategy.pricing.fast_venue_incoherent);

    let replay_decision = strategy.entry_submission_decision_at(replay.evaluation_now_ms);
    let snapshot = strategy
        .blocked_entry_strategy_input_evidence_snapshot_at(
            replay.evaluation_now_ms,
            &replay_decision,
        )
        .expect("replay should build blocked strategy-input evidence");
    assert_eq!(snapshot.spot_price, "108642.25");
    assert_eq!(
        snapshot.reference_current_price.as_deref(),
        Some("108500.25")
    );
    assert_eq!(
        snapshot.reference_current_price_source_id.as_deref(),
        Some(replay.reference.source_id.as_str())
    );
    assert_eq!(snapshot.reference_current_price_failed_over, Some(false));
    assert!(
        !snapshot.fast_venue_available,
        "fallback spot evidence must not be reported as admitted"
    );
    assert!(
        !snapshot.reference_current_price_available,
        "fallback reference evidence must not be reported as admitted"
    );
    let log_fields =
        strategy.entry_evaluation_log_fields_at(replay.evaluation_now_ms, &replay_decision);
    assert_eq!(log_fields.spot_price, Some(108642.25));
    assert_eq!(log_fields.reference_current_price, Some(108500.25));
    assert!(
        !log_fields.fast_venue_available,
        "raw signal observation should not change admitted fast-venue diagnostics"
    );
    assert!(
        !log_fields.reference_current_price_available,
        "fallback reference evidence must not be reported as admitted"
    );
    assert!(
        !log_fields.reference_current_price_available_without_fast_venue,
        "without-fast-venue marker should only track admitted reference state"
    );
    let ((), logs) = with_captured_strategy_logs(&strategy_id, || {
        strategy.log_entry_evaluation(
            replay.evaluation_now_ms,
            EntryEvaluationReceiveContext::new(LocalReceiveMs::new(replay.evaluation_now_ms)),
            &replay_decision,
        );
    });
    let entry_evaluation_logs = logs
        .iter()
        .filter(|(_, message)| {
            message.contains("binary_oracle_edge_taker entry evaluation:")
                && message.contains(&strategy_id)
        })
        .collect::<Vec<_>>();
    assert_eq!(
        entry_evaluation_logs.len(),
        1,
        "fallback replay should emit one entry-evaluation log for {strategy_id}: {logs:?}"
    );
    let entry_evaluation_log = &entry_evaluation_logs[0].1;
    assert!(
        entry_evaluation_log.contains("fast_venue_available=false"),
        "entry-evaluation log must expose fallback spot as not admitted: {entry_evaluation_log}"
    );
    assert!(
        entry_evaluation_log.contains("reference_current_price_available=false"),
        "entry-evaluation log must expose standalone fallback reference as not admitted: {entry_evaluation_log}"
    );
    assert!(
        entry_evaluation_log.contains("reference_current_price_available_without_fast_venue=false"),
        "entry-evaluation log must retain the without-fast-venue conjunction: {entry_evaluation_log}"
    );

    let submitted = strategy
        .try_submit_entry_order(replay.evaluation_now_ms)
        .expect("blocked replay entry should record skip evidence without submit error");
    assert_eq!(submitted, None);

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
        1,
        "replay should produce exactly one entry skip"
    );
    let skip = &entry_skips[0];
    assert!(
        skip.pricing_blocked_by.contains(
            &crate::bolt_v3_decision_evidence::BoltV3EntryPricingBlockReason::SpotPriceMissing
        ),
        "replay should preserve the incident blocker shape, got {:?}",
        skip.pricing_blocked_by
    );
    assert_eq!(skip.realized_vol.as_deref(), Some("0.287"));
    assert_eq!(
        skip.last_reference_ts_ms,
        Some(replay.reference.observed_ts_ms)
    );
    assert!(skip.fast_venue_incoherent);
    assert_eq!(skip.spot_price.as_deref(), Some("108642.25"));
    assert_eq!(skip.reference_current_price.as_deref(), Some("108500.25"));
    assert!(
        !skip.fast_venue_available,
        "fallback spot evidence must not be reported as admitted"
    );
    assert!(
        !skip.reference_current_price_available,
        "fallback reference evidence must not be reported as admitted"
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
        .try_submit_exit_order_for_trigger(
            1_200,
            ExitEvaluationTriggerContext::new(
                crate::bolt_v3_decision_evidence::BoltV3ExitTriggerSource::SelectionUpdate,
                1_200,
                None,
            ),
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
            .try_submit_exit_order_for_trigger(
                1_201,
                ExitEvaluationTriggerContext::new(
                    crate::bolt_v3_decision_evidence::BoltV3ExitTriggerSource::SelectionUpdate,
                    1_201,
                    None,
                ),
            )
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
        BoltV3ExitTriggerSource::SelectionUpdate
    );
    assert_eq!(exit_decisions[0].trigger_ts_event_ms, 1_200);
    assert_eq!(exit_decisions[0].trigger_ts_init_ms, None);
    assert_eq!(exit_decisions[0].rv_surface_id, TEST_SURFACE_ID);
    assert_eq!(exit_decisions[0].rv_snapshot_as_of_ms, Some(1_200));
    assert!(exit_decisions[0].rv_snapshot_ready);
    assert_eq!(exit_decisions[0].rv_snapshot_blockers, Vec::new());
    assert_eq!(
        exit_decisions[0].rv_gate_result,
        BoltV3ExitRvGateResult::MissingEvaluationEventTime
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
    let up_best_ask = strategy
        .active
        .books
        .up
        .best_ask
        .expect("ready-to-trade fixture should expose an up ask");
    let position = materialize_configured_position(
        &mut strategy,
        instrument_id,
        PositionId::from(stringify!(P_SHADOW_EXIT_FUTURE_RV)),
        position_quantity,
        up_best_ask,
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
            latest_accepted_receive_ms: Some(LocalReceiveMs::new(future_as_of_ms)),
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
    // The shared `ready_to_trade_strategy` fixture seeds the selected pricing
    // spot but not a reference current price (the two are independent pricing
    // inputs), so establish the reference observation this test depends on the
    // same way the pricing tests do before reading it back.
    strategy.pricing.observe_reference_current_price(&fast_spot(
        "bybit",
        3_100.5,
        exit_eval_now_ms,
    ));
    let signal_ask = strategy
        .pricing
        .last_reference_current_price()
        .expect("reference observation seeded above should carry a reference current price");
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
    // Freeze phase forces the position flat, so the recorded exit is a
    // forced-flat Exit: exit_evaluation_at short-circuits on
    // forced_flat_reasons before the RV gate, so the future-dated RV is
    // captured only as a diagnostic (rv_gate_result above), not as the exit
    // cause. RV-driven missing valuation input holds rather than liquidating by
    // default; that path is covered by the pricing / exposure tests.
    assert_eq!(decision.exit_decision, BoltV3ExitDecisionOutcome::Exit);
    assert_eq!(
        decision.forced_flat_reasons,
        vec![BoltV3ForcedFlatReason::Freeze]
    );
    assert_eq!(decision.blocked_reason, None);
    assert_eq!(decision.spot_price.as_deref(), Some("3100.25"));
    assert_eq!(
        decision.spot_venue_name.as_deref(),
        Some("signal_data_client")
    );
    assert!(decision.fast_venue_available);
    assert_eq!(decision.reference_current_price.as_deref(), Some("3100.5"));
    assert!(decision.reference_current_price_available);
    assert_eq!(decision.interval_open.as_deref(), Some("3100"));
    assert_eq!(decision.fair_probability_up, None);
    assert_eq!(decision.fair_probability_down, None);
    assert_eq!(decision.uncertainty_band_probability, None);
    assert!(
        decision.up_fee_bps.is_some(),
        "exit decision evidence must preserve the up-side fee input"
    );
    assert!(
        decision.down_fee_bps.is_some(),
        "exit decision evidence must preserve the down-side fee input"
    );
    assert!(
        decision.submission_order_side.is_some(),
        "exit decision evidence must preserve the submitted order side"
    );
    assert!(
        decision.submission_price.is_some(),
        "exit decision evidence must preserve the submitted order price"
    );
    assert!(
        decision.submission_quantity.is_some(),
        "exit decision evidence must preserve the submitted order quantity"
    );
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
            latest_accepted_receive_ms: Some(LocalReceiveMs::new(1_200)),
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
            latest_accepted_receive_ms: Some(LocalReceiveMs::new(1_200)),
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
fn blocked_strategy_input_evidence_records_state_transitions_not_ticks() {
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
    let initial_snapshot = crate::bolt_v3_realized_volatility::RealizedVolSnapshot {
        surface_id: TEST_SURFACE_ID.to_string(),
        as_of_ms: 1_200,
        latest_accepted_receive_ms: Some(LocalReceiveMs::new(1_200)),
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
    };
    strategy
        .pricing
        .observe_realized_vol_snapshot(initial_snapshot.clone());

    for now_ms in 1_200..=1_204 {
        assert_eq!(
            strategy
                .try_submit_entry_order(now_ms)
                .expect("same RV-not-ready state should not attempt submit"),
            None
        );
    }

    let mut changed_snapshot = initial_snapshot;
    changed_snapshot.as_of_ms = 1_205;
    changed_snapshot.blocked_reasons =
        vec![crate::bolt_v3_realized_volatility::RealizedVolBlockReason::SourceStale];
    strategy
        .pricing
        .observe_realized_vol_snapshot(changed_snapshot);
    assert_eq!(
        strategy
            .try_submit_entry_order(1_205)
            .expect("changed RV blocker state should not attempt submit"),
        None
    );

    let events = evidence.events();
    let blocked_snapshots = events
        .iter()
        .filter_map(|event| match event {
            RecordedDecisionEvidenceEvent::StrategyInput(snapshot)
                if snapshot.client_order_id.is_empty() =>
            {
                Some(snapshot)
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        blocked_snapshots.len(),
        2,
        "identical blocked evaluations must emit once and the RV blocker transition must emit the second record"
    );
    assert_eq!(
        blocked_snapshots[0].realized_volatility_blockers,
        vec!["quorum_not_ready".to_string()]
    );
    assert_eq!(
        blocked_snapshots[1].realized_volatility_blockers,
        vec!["source_stale".to_string()]
    );
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
    // RV not ready: the readiness-gated source path yields no usable RV, so the
    // entry-skip evidence carries no source ts. (The raw as_of_ms is still
    // recorded on the StrategyInput evidence above via the audit path.)
    assert_eq!(skip.realized_vol_source_ts_ms, None);
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
fn entry_skip_dedupe_records_liveness_state_transitions_not_price_ticks() {
    let evidence = Arc::new(RecordingSequencedDecisionEvidenceWriter::default());
    let submit_admission = submit_admission_with_provider_cap(Decimal::new(1, 2), evidence.clone());
    let mut strategy = ready_to_trade_strategy_with_decision_evidence_and_submit_admission(
        evidence.clone(),
        submit_admission,
    );
    register_test_strategy_with_active_instruments(&mut strategy);
    strategy.pricing.set_selected_pricing_spot(None);
    strategy.latest_signal_quote = None;

    let mut spot_missing = minimal_entry_submission_decision();
    spot_missing.evaluation.pricing_blocked_by = vec![EntryPricingBlockReason::SpotPriceMissing];

    strategy
        .record_entry_skip_once(
            1_200,
            &spot_missing,
            BoltV3EntrySkipReasonCategory::EntryPricingBlocked,
            None,
        )
        .expect("first spot-missing skip should record");

    strategy.latest_signal_quote = Some(fast_spot("bybit", 3_101.5, 1_201));
    strategy
        .record_entry_skip_once(
            1_201,
            &spot_missing,
            BoltV3EntrySkipReasonCategory::EntryPricingBlocked,
            None,
        )
        .expect("same blocker with price-only evidence changes should not error");

    let liveness_transition_ts_ms = strategy
        .active
        .last_reference_ts_ms
        .map_or(1_202, |last_reference_ts_ms| last_reference_ts_ms + 1);
    strategy.active.last_reference_ts_ms = Some(liveness_transition_ts_ms);
    strategy.pricing.observe_reference_current_price(&fast_spot(
        "chainlink",
        3_100.5,
        liveness_transition_ts_ms,
    ));
    strategy
        .record_entry_skip_once(
            1_202,
            &spot_missing,
            BoltV3EntrySkipReasonCategory::EntryPricingBlocked,
            None,
        )
        .expect("same blocker with a liveness-state transition should record");

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
        "same blocker should record the initial skip and the liveness-state transition only"
    );
    assert_eq!(entry_skips[0].spot_price, None);
    assert!(
        entry_skips[1].spot_price.is_some(),
        "liveness-state transition should still record current spot evidence when present"
    );
    assert_eq!(
        entry_skips[1].reference_current_price.as_deref(),
        Some("3100.5")
    );
    assert_eq!(
        entry_skips[1].last_reference_ts_ms,
        Some(liveness_transition_ts_ms)
    );
}

#[test]
fn entry_skip_dedupe_does_not_record_every_reference_tick_while_blocked() {
    let replay = strategy_input_quote_replay();
    let evidence = Arc::new(RecordingSequencedDecisionEvidenceWriter::default());
    let submit_admission = submit_admission_with_provider_cap(Decimal::new(1, 2), evidence.clone());
    let mut strategy = ready_to_trade_strategy_with_decision_evidence_and_submit_admission(
        evidence.clone(),
        submit_admission,
    );
    strategy.config.reference_current_price = Some(replay_reference_price_config(&replay));
    strategy.apply_selection_snapshot(active_snapshot_with_start("MKT-1", replay.market_start_ms));
    strategy.active.price_to_beat = Some(replay.reference.price);
    strategy.active.interval_open = Some(replay.reference.price);
    strategy.pricing.set_selected_pricing_spot(None);
    strategy.latest_signal_quote = None;
    let (cache, clock) = register_test_strategy_with_clock(&mut strategy);
    add_active_instruments_to_cache(&strategy, &cache);

    let mut spot_missing = minimal_entry_submission_decision();
    spot_missing.evaluation.pricing_blocked_by = vec![EntryPricingBlockReason::SpotPriceMissing];

    let first_reference_ts_ms = replay.reference.observed_ts_ms;
    let first_received_ts_ms = replay.reference.received_ts_ms;
    for (index, (price, observed_ts_ms, received_ts_ms)) in [
        (108_500.25, first_reference_ts_ms, first_received_ts_ms),
        (
            108_501.25,
            first_reference_ts_ms.saturating_add(20),
            first_received_ts_ms.saturating_add(20),
        ),
        (
            108_502.25,
            first_reference_ts_ms.saturating_add(40),
            first_received_ts_ms.saturating_add(40),
        ),
    ]
    .into_iter()
    .enumerate()
    {
        clock.borrow_mut().set_time(UnixNanos::from(
            received_ts_ms.saturating_mul(NANOS_PER_MILLI_U64),
        ));
        let update = replay_reference_update_at(&replay, price, observed_ts_ms, received_ts_ms);
        DataActor::on_data(&mut strategy, &update)
            .expect("reference quote update should be accepted");
        assert_eq!(
            strategy.active.reference_current_price,
            Some(price),
            "precondition: reference update {index} should be admitted"
        );
        assert_eq!(
            strategy.active.last_reference_ts_ms,
            Some(observed_ts_ms),
            "precondition: reference update {index} should advance the accepted quote timestamp"
        );

        strategy
            .record_entry_skip_once(
                received_ts_ms,
                &spot_missing,
                BoltV3EntrySkipReasonCategory::EntryPricingBlocked,
                None,
            )
            .expect("blocked entry skip should record or dedupe without error");
    }

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
        1,
        "accepted reference ticks in one blocked interval must not re-record every tick"
    );
    assert_eq!(
        entry_skips[0].last_reference_ts_ms,
        Some(first_reference_ts_ms),
        "bounded dedupe should keep the first skip evidence for an unchanged liveness state"
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
        realized_volatility_receipt: EntryRealizedVolatilityReceipt {
            gate_result: BoltV3RvGateResult::MissingSnapshot,
            receive_watermark_ms: None,
            realized_vol: None,
            source_venue: None,
            source_ts_ms: None,
            evidence: RealizedVolatilityEvidenceFields {
                surface_id: String::new(),
                as_of_ms: None,
                annualized_decimal: String::new(),
                measured_annualized_decimal: String::new(),
                noise_robust_annualized_decimal: String::new(),
                continuous_annualized_decimal: String::new(),
                jump_annualized_decimal: String::new(),
                forecast_annualized_decimal: String::new(),
                pricing_component: String::new(),
                seconds_per_annum: String::new(),
                aggregation: String::new(),
                sources_used: Vec::new(),
                source_diagnostics: Vec::new(),
                unknown_source_rejections: BTreeMap::new(),
                blockers: Vec::new(),
                config_fingerprint: String::new(),
            },
        },
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

/// Collect every recorded exit-decision evidence record, in order.
fn recorded_exit_decisions(
    evidence: &RecordingSequencedDecisionEvidenceWriter,
) -> Vec<crate::bolt_v3_decision_evidence::BoltV3ExitDecisionEvidence> {
    evidence
        .events()
        .into_iter()
        .filter_map(|event| match event {
            RecordedDecisionEvidenceEvent::ExitDecision(evidence) => Some(evidence),
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
        .try_submit_exit_order_for_trigger(
            1_200,
            ExitEvaluationTriggerContext::new(
                crate::bolt_v3_decision_evidence::BoltV3ExitTriggerSource::SignalQuote,
                1_200,
                Some(1_220),
            ),
        )
        .expect("control exit evaluation should not error with a ready realized-vol surface");

    let failing_evidence = Arc::new(ExitEvaluationFailingDecisionEvidenceWriter::default());
    let mut failing_strategy =
        exit_evidence_strategy_with_open_position_using_writer(failing_evidence.clone());
    failing_strategy
        .pricing
        .seed_ready_realized_vol(Some("<SOURCE_ID>".to_string()), 1.5, 1_200);
    let failing_result = failing_strategy
        .try_submit_exit_order_for_trigger(
            1_200,
            ExitEvaluationTriggerContext::new(
                crate::bolt_v3_decision_evidence::BoltV3ExitTriggerSource::SignalQuote,
                1_200,
                Some(1_220),
            ),
        )
        .expect("a failing exit-evaluation sink must be swallowed, not propagated");

    // The trading-side result is structurally identical with and without the sink
    // failure (the client order id itself is a fresh UUID per run, so compare
    // whether a submit occurred (Some vs None), not the minted id).
    assert_eq!(control_result.is_some(), failing_result.is_some());
    // The swallow path was exercised: the sink was reached and did error.
    assert_eq!(
        failing_evidence.exit_evaluation_attempts(),
        1,
        "the exit-evaluation sink must have been attempted exactly once"
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

#[test]
fn exit_decision_evidence_reports_fast_venue_when_position_spot_is_absent() {
    let (mut strategy, evidence) = exit_evidence_strategy_with_open_position();
    strategy.active.market_id = Some("different-active-market".to_string());

    strategy
        .record_exit_decision_once(
            1_200,
            ExitEvaluationTriggerContext::new(
                crate::bolt_v3_decision_evidence::BoltV3ExitTriggerSource::SignalQuote,
                1_200,
                Some(1_180),
            ),
            &minimal_exit_submission_decision(),
        )
        .expect("exit-decision evidence should record");

    let records = recorded_exit_decisions(&evidence);
    assert_eq!(records.len(), 1);
    let record = &records[0];
    assert_eq!(
        record.spot_price, None,
        "the position-coupled exit spot price should be absent for a market mismatch"
    );
    assert!(
        record.fast_venue_available,
        "fast_venue_available must report selected venue state, not position-coupled price presence"
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
        .pricing
        .observe_reference_current_price(&fast_spot("bybit", 3_099.75, 1_200));

    strategy
        .try_submit_exit_order_for_trigger(
            1_200,
            ExitEvaluationTriggerContext::new(
                crate::bolt_v3_decision_evidence::BoltV3ExitTriggerSource::SignalQuote,
                1_200,
                Some(1_220),
            ),
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
    assert_eq!(record.trigger_ts_init_ms, Some(1_220));
    assert_eq!(record.spot_price.as_deref(), Some("3099.75"));
    assert_eq!(record.spot_venue_name.as_deref(), Some("bybit"));
    assert!(record.fast_venue_available);
    assert_eq!(record.reference_current_price.as_deref(), Some("3099.75"));
    assert!(record.reference_current_price_available);
    assert_eq!(record.interval_open.as_deref(), Some("3100"));
    assert!(
        record.fair_probability_up.is_some(),
        "exit evaluation evidence must preserve the computed fair probability"
    );
    assert!(
        record.fair_probability_down.is_some(),
        "exit evaluation evidence must preserve the computed complement probability"
    );
    assert!(
        record.uncertainty_band_probability.is_some(),
        "exit evaluation evidence must preserve the computed uncertainty band"
    );
    assert!(
        record.up_fee_bps.is_some(),
        "exit evaluation evidence must preserve the up-side fee input"
    );
    assert!(
        record.down_fee_bps.is_some(),
        "exit evaluation evidence must preserve the down-side fee input"
    );
}

#[test]
fn exit_evaluation_evidence_reports_fast_venue_when_position_spot_is_absent() {
    let (mut strategy, evidence) = exit_evidence_strategy_with_open_position();
    strategy.active.market_id = Some("different-active-market".to_string());

    strategy.record_exit_evaluation_evidence(
        1_200,
        &minimal_exit_submission_decision(),
        ExitEvaluationTriggerContext::new(
            crate::bolt_v3_decision_evidence::BoltV3ExitTriggerSource::SignalQuote,
            1_200,
            Some(1_180),
        ),
        false,
    );

    let records = recorded_exit_evaluations(&evidence);
    assert_eq!(records.len(), 1);
    let record = &records[0];
    assert_eq!(
        record.spot_price, None,
        "the position-coupled exit spot price should be absent for a market mismatch"
    );
    assert!(
        record.fast_venue_available,
        "fast_venue_available must report selected venue state, not position-coupled price presence"
    );
}

#[test]
fn exit_evaluation_evidence_omits_non_finite_optional_numbers() {
    let (mut strategy, evidence) = exit_evidence_strategy_with_open_position();
    let mut decision = minimal_exit_submission_decision();
    decision.evaluation.hold_ev_bps = Some(f64::NAN);
    decision.evaluation.exit_ev_bps = Some(f64::INFINITY);
    decision.price = Some(f64::NAN);

    strategy.record_exit_evaluation_evidence(
        1_200,
        &decision,
        ExitEvaluationTriggerContext::new(
            crate::bolt_v3_decision_evidence::BoltV3ExitTriggerSource::SignalQuote,
            1_200,
            Some(1_180),
        ),
        false,
    );

    let records = recorded_exit_evaluations(&evidence);
    assert_eq!(records.len(), 1);
    let record = &records[0];
    assert_eq!(record.hold_ev_bps, None);
    assert_eq!(record.exit_ev_bps, None);
    assert_eq!(record.submission_price, None);
}

#[test]
fn exit_evaluation_evidence_records_future_dated_rv_gate_with_delta() {
    let (mut strategy, evidence) = exit_evidence_strategy_with_open_position();
    // A future receive watermark is still rejected within the one valid clock
    // domain. The independent venue-event delta remains diagnostic only.
    strategy
        .pricing
        .seed_ready_realized_vol(Some("<SOURCE_ID>".to_string()), 1.5, 2_000);

    strategy
        .try_submit_exit_order_for_trigger(
            1_200,
            ExitEvaluationTriggerContext::new(
                crate::bolt_v3_decision_evidence::BoltV3ExitTriggerSource::BookDelta,
                1_190,
                Some(1_190),
            ),
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
        Some(810),
        "the durable record must capture the as_of-minus-trigger-event delta for RCA"
    );
}

#[test]
fn exit_evaluation_evidence_accepts_local_trigger_with_receive_time() {
    let (mut strategy, evidence) = exit_evidence_strategy_with_open_position();
    strategy.active.phase = SelectionPhase::Active;
    strategy
        .pricing
        .seed_ready_realized_vol(Some("<SOURCE_ID>".to_string()), 1.5, 1_200);

    strategy
        .try_submit_exit_order_for_trigger(1_200, ExitEvaluationTriggerContext::unknown(1_200))
        .expect("exit evaluation should not error when a local trigger has receive time");

    let records = recorded_exit_evaluations(&evidence);
    assert_eq!(
        records.len(),
        1,
        "a local-clock exit evaluation must record exactly one durable evidence record"
    );
    let record = &records[0];
    assert_eq!(
        record.rv_gate_result,
        crate::bolt_v3_decision_evidence::BoltV3RvGateResult::Accepted,
        "local triggers must use their receive-domain evaluation timestamp"
    );
    assert_eq!(record.rv_as_of_ms, Some(1_200));
    assert_eq!(record.rv_as_of_minus_now_ms, None);
}

#[test]
fn exit_evaluation_evidence_holds_when_receive_time_is_structurally_absent() {
    let (mut strategy, evidence) = exit_evidence_strategy_with_open_position();
    strategy.active.phase = SelectionPhase::Active;
    strategy
        .pricing
        .seed_ready_realized_vol(Some("<SOURCE_ID>".to_string()), 1.5, 1_200);

    strategy
        .try_submit_exit_order_for_trigger(
            1_200,
            ExitEvaluationTriggerContext::new(
                crate::bolt_v3_decision_evidence::BoltV3ExitTriggerSource::Other,
                1_200,
                None,
            ),
        )
        .expect("missing receive context should fail closed without an error");

    let records = recorded_exit_evaluations(&evidence);
    assert_eq!(records.len(), 1);
    let record = &records[0];
    assert_eq!(
        record.rv_gate_result,
        crate::bolt_v3_decision_evidence::BoltV3RvGateResult::MissingEvaluationEventTime
    );
    assert_eq!(
        record.exit_decision,
        crate::bolt_v3_decision_evidence::BoltV3ExitDecisionOutcome::Hold,
        "missing receive-domain input must hold, never liquidate by default"
    );
}

#[test]
fn exit_evaluation_dedupe_does_not_oscillate_across_trigger_sources() {
    let (mut strategy, evidence) = exit_evidence_strategy_with_open_position();
    strategy
        .pricing
        .seed_ready_realized_vol(Some("<SOURCE_ID>".to_string()), 1.5, 1_200);
    let decision = minimal_exit_submission_decision();

    for trigger_context in [
        ExitEvaluationTriggerContext::new(
            crate::bolt_v3_decision_evidence::BoltV3ExitTriggerSource::BookDelta,
            1_200,
            Some(1_210),
        ),
        ExitEvaluationTriggerContext::new(
            crate::bolt_v3_decision_evidence::BoltV3ExitTriggerSource::SelectionUpdate,
            1_220,
            Some(1_220),
        ),
        ExitEvaluationTriggerContext::new(
            crate::bolt_v3_decision_evidence::BoltV3ExitTriggerSource::SignalQuote,
            1_200,
            Some(1_230),
        ),
        ExitEvaluationTriggerContext::new(
            crate::bolt_v3_decision_evidence::BoltV3ExitTriggerSource::SelectionUpdate,
            1_240,
            Some(1_240),
        ),
    ] {
        strategy.record_exit_evaluation_evidence(1_240, &decision, trigger_context, false);
    }

    let records = recorded_exit_evaluations(&evidence);
    assert_eq!(
        records.len(),
        1,
        "unchanged pricing state must produce one exit-evaluation transition across book, signal, and selection triggers"
    );
}

#[test]
fn exit_evaluation_dedupe_ignores_alternating_consuming_venue_clock_lead() {
    let (mut strategy, evidence) = exit_evidence_strategy_with_open_position();
    strategy
        .pricing
        .seed_ready_realized_vol(Some("<SOURCE_ID>".to_string()), 1.5, 1_200);
    let decision = minimal_exit_submission_decision();

    for (index, event_ts_ms) in [1_200, 1_199, 1_200, 1_199, 1_200, 1_199]
        .into_iter()
        .enumerate()
    {
        strategy.record_exit_evaluation_evidence(
            1_300,
            &decision,
            ExitEvaluationTriggerContext::new(
                crate::bolt_v3_decision_evidence::BoltV3ExitTriggerSource::BookDelta,
                event_ts_ms,
                Some(1_210 + index as u64),
            ),
            false,
        );
    }

    let records = recorded_exit_evaluations(&evidence);
    assert_eq!(
        records.len(),
        1,
        "six unchanged book evaluations must collapse even when an independent venue clock alternately leads and trails the RV venue clock"
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
        .try_submit_exit_order_for_trigger(
            1_200,
            ExitEvaluationTriggerContext::new(
                crate::bolt_v3_decision_evidence::BoltV3ExitTriggerSource::SignalQuote,
                1_200,
                Some(1_200),
            ),
        )
        .expect("first shadow exit should pass evidence and admission");

    // The position is now latched as ExitPending. Drive four MORE evaluations: every
    // one produces the identical latched outcome key (Hold / exit_already_pending /
    // Accepted). The first latched tick is a key change (one record); the remaining
    // three are identical and MUST be suppressed by the flood guard.
    for tick in 1_201..=1_204 {
        assert_eq!(
            strategy
                .try_submit_exit_order_for_trigger(
                    tick,
                    ExitEvaluationTriggerContext::new(
                        crate::bolt_v3_decision_evidence::BoltV3ExitTriggerSource::SignalQuote,
                        tick,
                        Some(tick),
                    ),
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
