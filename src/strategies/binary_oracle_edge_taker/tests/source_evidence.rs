#![cfg(test)]

use super::shared_fixture::{unique_log_capture_strategy_id, with_captured_strategy_logs};
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

#[test]
fn novelty_registry_has_a_production_mapping_for_every_declared_state() {
    use crate::bolt_v3_evidence_novelty::{
        EVIDENCE_STATE_REGISTRATIONS, EvidenceCanonicalState, EvidenceStateOwner,
    };

    let registered = |owner| {
        EVIDENCE_STATE_REGISTRATIONS
            .iter()
            .filter(|registration| registration.owner == owner)
            .map(|registration| registration.state)
            .collect::<std::collections::BTreeSet<_>>()
    };

    for owner in EvidenceStateOwner::ALL {
        let (production_states, expected_count) = match production_novelty_domain(*owner) {
            EvidenceNoveltyProductionDomain::BlockedStrategyInputSnapshot => {
                let states = EvidenceRvGateResult::ALL
                    .iter()
                    .copied()
                    .flat_map(|gate_result| {
                        [false, true].map(|watermark_present| {
                            blocked_strategy_input_canonical_state(gate_result, watermark_present)
                        })
                    })
                    .collect::<std::collections::BTreeSet<EvidenceCanonicalState>>();
                (states, EvidenceRvGateResult::ALL.len() * 2)
            }
            EvidenceNoveltyProductionDomain::EntrySkip => {
                let states = EvidenceEntrySkipReason::ALL
                    .iter()
                    .copied()
                    .map(entry_skip_canonical_state)
                    .collect::<std::collections::BTreeSet<_>>();
                (states, EvidenceEntrySkipReason::ALL.len())
            }
        };
        assert_eq!(
            production_states.len(),
            expected_count,
            "every production state must map injectively"
        );
        assert_eq!(
            production_states,
            registered(*owner),
            "every registered owner must have one exhaustive production domain"
        );
    }
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

const LOG_CAPTURE_CHILD_ENV: &str = "BOLT_TAKER_SOURCE_EVIDENCE_LOG_CAPTURE";

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
    let (result, logs) = with_captured_strategy_logs(strategy_id, action);
    let matching = logs
        .into_iter()
        .filter(|(_, message)| message.contains(failure_message))
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
    let decision_evidence = recording_decision_evidence();
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
    .with_order_economics(fixture_order_economics())
    .with_realized_volatility_surfaces(surfaces);
    let mut strategy = BinaryOracleEdgeTaker::new(config, context);
    register_test_strategy(&mut strategy);
    strategy
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

    strategy.observe_reference_snapshot(&reference_tick(900, 3_100.0), LocalReceiveMs::new(900));
    assert!(strategy.active.interval_open.is_none());

    strategy
        .observe_reference_snapshot(&reference_tick(1_000, 3_101.0), LocalReceiveMs::new(1_000));
    assert_eq!(strategy.active.interval_open, Some(3_099.0));
}

#[test]
fn interval_open_does_not_use_reference_price_without_source_bound_price_to_beat() {
    let mut strategy = test_strategy();
    strategy.apply_selection_snapshot(active_snapshot_with_start("MKT-1", 1_000));

    strategy.observe_reference_snapshot(
        &ReferenceSnapshot {
            ts_ms: 1_000,
            topic: "platform.reference.test.spot".to_string(),
            fair_value: Some(3_107.0),
            confidence: 1.0,
            venues: vec![
                oracle_venue("reference", 1.0, 3_100.0, 1_000),
                orderbook_venue("bybit", 0.9, 3_120.0, 1_000),
            ],
        },
        LocalReceiveMs::new(1_000),
    );

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

    strategy.observe_reference_snapshot(
        &ReferenceSnapshot {
            ts_ms: 1_000,
            topic: "platform.reference.test.spot".to_string(),
            fair_value: Some(3_107.0),
            confidence: 1.0,
            venues: vec![
                oracle_venue("reference", 1.0, 3_100.0, 1_000),
                orderbook_venue("bybit", 0.9, 3_120.0, 1_000),
            ],
        },
        LocalReceiveMs::new(1_000),
    );

    assert_eq!(strategy.active.interval_open, Some(3_099.0));
}

#[test]
fn interval_open_does_not_use_fused_reference_without_source_bound_price_to_beat() {
    let mut strategy = test_strategy();
    strategy.apply_selection_snapshot(active_snapshot_with_start("MKT-1", 1_000));

    strategy.observe_reference_snapshot(
        &ReferenceSnapshot {
            ts_ms: 1_000,
            topic: "platform.reference.test.spot".to_string(),
            fair_value: Some(3_107.0),
            confidence: 1.0,
            venues: vec![],
        },
        LocalReceiveMs::new(1_000),
    );

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
    let evidence = recording_decision_evidence();
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
    strategy.observe_reference_snapshot(
        &ReferenceSnapshot {
            ts_ms: 1_200,
            topic: "platform.reference.test.spot".to_string(),
            fair_value: Some(3_100.5),
            confidence: 1.0,
            venues: vec![
                oracle_venue("reference", 1.0, 3_100.5, 1_200),
                orderbook_venue("bybit", 0.9, 3_100.5, 1_200),
            ],
        },
        LocalReceiveMs::new(1_200),
    );
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

    let events = evidence
        .recorded_facts()
        .expect("recorded current evidence must decode");
    let [
        CurrentFact::SubmitLinkedStrategyInputSnapshot(snapshot),
        CurrentFact::EntryOrderIntent(intent),
        CurrentFact::RejectedEntryAdmission(admission),
        CurrentFact::OrderReject(reject),
    ] = events.as_slice()
    else {
        panic!(
            "expected strategy input, order intent, admission, order-reject sequence; got {events:#?}"
        );
    };

    let details = &snapshot.details;
    assert_eq!(details.strategy_id, strategy.config.strategy_id);
    assert_eq!(details.price_to_beat_value, "3100");
    assert_eq!(details.reference_quote_ts_event, 1_200);
    assert_eq!(details.spot_price, "3100.5");
    let StrategyInputRvState::Present {
        selected_annualized_decimal,
        ..
    } = &details.realized_volatility
    else {
        panic!("admitted snapshot must carry present realized volatility");
    };
    assert_eq!(selected_annualized_decimal.as_deref(), Some("1.5"));
    assert_eq!(details.seconds_to_market_end, 300);
    assert_eq!(details.market_id.as_deref(), Some("MKT-1"));
    assert_eq!(
        details.polymarket_condition_id.as_deref(),
        Some("condition-MKT-1")
    );
    assert_eq!(
        details.polymarket_market_slug.as_deref(),
        Some("slug-MKT-1")
    );
    assert_eq!(
        details.polymarket_question_id.as_deref(),
        Some("question-MKT-1")
    );
    assert_eq!(
        details.up_instrument_id.as_deref(),
        Some("condition-MKT-1-MKT-1-UP.POLYMARKET")
    );
    assert_eq!(
        details.down_instrument_id.as_deref(),
        Some("condition-MKT-1-MKT-1-DOWN.POLYMARKET")
    );
    assert_eq!(details.selected_side, Some(EvidenceOutcomeSide::Up));
    assert!(
        details
            .up_worst_case_edge_basis_points
            .as_deref()
            .and_then(|value| value.parse::<f64>().ok())
            .is_some(),
        "admitted entry snapshot must preserve the up-side thin margin"
    );
    assert!(
        details
            .down_worst_case_edge_basis_points
            .as_deref()
            .and_then(|value| value.parse::<f64>().ok())
            .is_some(),
        "admitted entry snapshot must preserve the down-side thin margin"
    );
    assert!(details.gate_blocked_by.is_empty());
    assert!(details.pricing_blocked_by.is_empty());
    assert_eq!(details.fast_venue_name.as_deref(), Some("bybit"));
    assert!(
        details.fast_venue_available,
        "admitted entry snapshot must expose admitted spot state"
    );
    assert!(
        details.reference_current_price_available,
        "admitted entry snapshot must expose admitted reference state"
    );
    assert_eq!(details.fast_venue_age_ms, Some(17));
    assert_eq!(details.fast_venue_jitter_ms, Some(3));
    assert!(!details.fast_venue_incoherent);
    assert_eq!(
        details
            .lead_agreement_corr
            .as_deref()
            .and_then(|value| value.parse::<f64>().ok()),
        Some(0.99)
    );
    assert_eq!(
        snapshot.submission.instrument_id,
        intent.details.instrument_id
    );
    assert_eq!(snapshot.submission.order_side, intent.details.order_side);
    assert_eq!(snapshot.submission.price, intent.details.price);
    assert_eq!(snapshot.submission.quantity, intent.details.quantity);
    assert_eq!(
        snapshot.submission.client_order_id,
        intent.details.client_order_id
    );
    assert_eq!(
        admission.details.client_order_id,
        intent.details.client_order_id
    );
    assert_eq!(
        admission.reason,
        crate::bolt_v3_current_evidence::AdmissionRejectionReason::NotionalCapExceeded
    );
    assert_eq!(reject.client_order_id, intent.details.client_order_id);
    assert_eq!(
        reject.reject_source,
        crate::bolt_v3_current_evidence::OrderRejectSource::SubmitAdmission
    );
    assert_eq!(
        reject.reject_reason,
        crate::bolt_v3_current_evidence::OrderRejectReason::AdmissionRejected
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
    assert_eq!(fields.realized_vol_gate_result, RvGateResult::Accepted);
    assert_eq!(
        fields.realized_vol_receive_watermark_ms,
        Some(LocalReceiveMs::new(1_200))
    );
    assert_admitted_a_snapshot_fields(&fields.realized_volatility_evidence);
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
        .record_entry_skip_once(1_300, &decision, EntrySkipReason::NoSideSelected)
        .expect("skip evidence should record from the admitted receipt");

    let events = evidence
        .recorded_facts()
        .expect("recorded current evidence must decode");
    let Some(CurrentFact::EntrySkipObservation(skip)) = events.first() else {
        panic!("expected entry-skip evidence; got {events:#?}");
    };
    assert_eq!(skip.realized_vol.as_deref(), Some("1.5"));
    assert_eq!(
        skip.realized_vol_source_venue.as_deref(),
        Some(TEST_SOURCE_ID)
    );
    assert_eq!(skip.realized_vol_source_ts_ms, Some(1_200));
    assert_eq!(skip.realized_vol_gate_result, Some(RvGateResult::Accepted));
    assert_eq!(skip.realized_vol_receive_watermark_ms, Some(1_200));
    let snapshot = skip
        .realized_vol_snapshot
        .as_ref()
        .expect("durable skip must independently identify admitted snapshot A");
    assert_eq!(snapshot.surface_id, TEST_SURFACE_ID);
    assert_eq!(snapshot.as_of_ms, Some(1_200));
    assert_eq!(snapshot.annualized_decimal.as_deref(), Some("1.5"));
    assert_eq!(snapshot.measured_annualized_decimal.as_deref(), Some("1.5"));
    assert_eq!(
        snapshot.noise_robust_annualized_decimal.as_deref(),
        Some("1.5")
    );
    assert_eq!(
        snapshot.continuous_annualized_decimal.as_deref(),
        Some("1.5")
    );
    assert_eq!(snapshot.jump_annualized_decimal.as_deref(), Some("0"));
    assert_eq!(snapshot.forecast_annualized_decimal, None);
    assert_eq!(
        snapshot.pricing_component,
        RealizedVolPricingComponent::Measured
    );
    assert_eq!(snapshot.seconds_per_annum, "31536000");
    assert_eq!(snapshot.aggregation, RealizedVolAggregation::UpperQuantile);
    assert_eq!(snapshot.sources_used, vec![TEST_SOURCE_ID.to_string()]);
    assert!(snapshot.source_diagnostics.is_empty());
    assert!(snapshot.unknown_source_rejections.is_empty());
    assert!(snapshot.blockers.is_empty());
    assert_eq!(
        snapshot.config_fingerprint,
        "<test-seed-config-fingerprint>"
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
    assert!(
        matches!(&strategy.exposure, ExposureState::PendingEntry(_)),
        "submitted routing must retain pending entry exposure"
    );
    let events = evidence
        .recorded_facts()
        .expect("recorded current evidence must decode");
    assert!(
        events
            .iter()
            .any(|event| matches!(event, CurrentFact::EntryOrderIntent(_))),
        "actual submit routing must persist an order intent"
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, CurrentFact::AdmittedEntryAdmission(_))),
        "actual submit routing must persist accepted submit admission"
    );
    let snapshot = events
        .into_iter()
        .find_map(|event| match event {
            CurrentFact::SubmitLinkedStrategyInputSnapshot(snapshot) => Some(snapshot),
            _ => None,
        })
        .expect("actual submit route must persist strategy-input evidence");
    let StrategyInputRvState::Present {
        selected_annualized_decimal,
        gate_result,
        receive_watermark_ms,
        snapshot,
    } = &snapshot.details.realized_volatility
    else {
        panic!("submit-linked snapshot must carry present realized volatility");
    };
    assert_eq!(selected_annualized_decimal.as_deref(), Some("1.5"));
    assert_eq!(snapshot.surface_id, TEST_SURFACE_ID);
    assert_eq!(snapshot.as_of_ms, Some(1_200));
    assert_eq!(snapshot.sources_used, vec![TEST_SOURCE_ID.to_string()]);
    assert_eq!(*gate_result, RvGateResult::Accepted);
    assert_eq!(*receive_watermark_ms, Some(1_200));
    assert_eq!(snapshot.annualized_decimal.as_deref(), Some("1.5"));
    assert_eq!(snapshot.measured_annualized_decimal.as_deref(), Some("1.5"));
    assert_eq!(
        snapshot.noise_robust_annualized_decimal.as_deref(),
        Some("1.5")
    );
    assert_eq!(
        snapshot.continuous_annualized_decimal.as_deref(),
        Some("1.5")
    );
    assert_eq!(snapshot.jump_annualized_decimal.as_deref(), Some("0"));
    assert_eq!(snapshot.forecast_annualized_decimal, None);
    assert_eq!(
        snapshot.pricing_component,
        RealizedVolPricingComponent::Measured
    );
    assert_eq!(snapshot.seconds_per_annum, "31536000");
    assert_eq!(snapshot.aggregation, RealizedVolAggregation::UpperQuantile);
    assert!(snapshot.source_diagnostics.is_empty());
    assert!(snapshot.unknown_source_rejections.is_empty());
    assert!(snapshot.blockers.is_empty());
    assert_eq!(
        snapshot.config_fingerprint,
        "<test-seed-config-fingerprint>"
    );
}

#[test]
fn submit_snapshot_failure_clears_pending_entry_and_never_reaches_submit_admission() {
    let evidence = failing_decision_evidence();
    let submit_admission = Arc::new(
        crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new(evidence.clone()),
    );
    let mut strategy = ready_to_trade_strategy_with_decision_evidence_and_submit_admission(
        evidence,
        submit_admission.clone(),
    );
    register_test_strategy_with_active_instruments(&mut strategy);
    strategy
        .pricing
        .seed_ready_realized_vol(Some(TEST_SOURCE_ID.to_string()), 1.5, 1_200);
    let decision = strategy.entry_submission_decision_at(1_200);
    assert!(
        decision.instrument_id.is_some(),
        "fixture must reach admitted entry order construction"
    );

    let error = strategy
        .submit_admitted_entry_decision(1_200, decision)
        .expect_err("submit-linked snapshot failure must veto the entry");

    assert!(
        error
            .to_string()
            .contains("evidence commit indeterminate during write"),
        "{error:#}"
    );
    assert!(
        !matches!(strategy.exposure, ExposureState::PendingEntry(_)),
        "snapshot failure must clear strategy-local pending-entry state"
    );
    assert_eq!(
        submit_admission.admitted_order_count(),
        0,
        "snapshot failure must stop before shared submit admission and NT submit"
    );
}

fn assert_admitted_a_snapshot_fields(fields: &RealizedVolatilityEvidenceFields) {
    assert_eq!(fields.surface_id, TEST_SURFACE_ID);
    assert_eq!(fields.as_of_ms, Some(1_200));
    assert_eq!(fields.annualized_decimal.as_deref(), Some("1.5"));
    assert_eq!(fields.measured_annualized_decimal.as_deref(), Some("1.5"));
    assert_eq!(
        fields.noise_robust_annualized_decimal.as_deref(),
        Some("1.5")
    );
    assert_eq!(fields.continuous_annualized_decimal.as_deref(), Some("1.5"));
    assert_eq!(fields.jump_annualized_decimal.as_deref(), Some("0"));
    assert_eq!(fields.forecast_annualized_decimal, None);
    assert_eq!(
        fields.pricing_component,
        Some(RealizedVolPricingComponent::Measured)
    );
    assert_eq!(fields.seconds_per_annum, "31536000");
    assert_eq!(
        fields.aggregation,
        Some(RealizedVolAggregation::UpperQuantile)
    );
    assert_eq!(fields.sources_used, vec![TEST_SOURCE_ID.to_string()]);
    assert!(fields.source_diagnostics.is_empty());
    assert!(fields.unknown_source_rejections.is_empty());
    assert!(fields.blockers.is_empty());
    assert_eq!(fields.config_fingerprint, "<test-seed-config-fingerprint>");
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
    replacement.source_diagnostics = vec![
        crate::bolt_v3_realized_volatility::RealizedVolSourceDiagnostic {
            source_id: TEST_SOURCE_ID_B.to_string(),
            source_class: crate::bolt_v3_realized_volatility::RealizedVolSourceClass::Trade,
            sample_kind: crate::bolt_v3_realized_volatility::RealizedVolSampleKind::Trade,
            enabled: false,
            counts_toward_quorum: false,
            status: crate::bolt_v3_realized_volatility::RealizedVolSourceStatus::Blocked,
            annualized_realized_vol_decimal: Some(8.9),
            measured_annualized_realized_vol_decimal: Some(8.8),
            noise_robust_annualized_realized_vol_decimal: Some(8.7),
            continuous_annualized_realized_vol_decimal: Some(8.6),
            jump_annualized_realized_vol_decimal: Some(8.5),
            first_sample_ts_ms: Some(1_250),
            last_sample_ts_ms: Some(1_300),
            raw_sample_count: 7,
            grid_sample_count: 6,
            coverage_ratio: 0.75,
            max_inter_sample_gap_ms: Some(50),
            last_rejected_reason: Some(
                crate::bolt_v3_realized_volatility::RealizedVolSourceRejectReason::InvalidPrice,
            ),
            last_rejected_event_ts_ms: Some(1_299),
            last_rejected_recv_ts_ms: Some(1_301),
            rejection_counters: [(
                crate::bolt_v3_realized_volatility::RealizedVolSourceRejectReason::InvalidPrice,
                3,
            )]
            .into_iter()
            .collect(),
            block_reason: Some(
                crate::bolt_v3_realized_volatility::RealizedVolBlockReason::SourceStale,
            ),
        },
    ];
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
    Arc<crate::bolt_v3_current_evidence::DecisionEvidenceRecorder>,
) {
    let evidence = recording_decision_evidence();
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
    let evidence = recording_decision_evidence();
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
        replay.evaluation_now_ms,
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
    strategy.apply_reference_price_selection_at(replay.evaluation_now_ms);
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
    let record_outcome =
        recording_decision_evidence().record_blocked_strategy_input_observation(snapshot.clone());
    assert!(
        matches!(
            record_outcome,
            crate::bolt_v3_current_evidence::ObservationRecordOutcome::Appended(_)
        ),
        "a snapshot built by the blocked producer must be recordable: {record_outcome:?}"
    );
    assert_eq!(snapshot.details.spot_price.as_deref(), Some("108642.25"));
    assert_eq!(
        snapshot.details.reference_current_price.as_deref(),
        Some("108500.25")
    );
    assert_eq!(
        snapshot
            .details
            .reference_current_price_source_id
            .as_deref(),
        Some(replay.reference.source_id.as_str())
    );
    assert_eq!(
        snapshot.details.reference_current_price_failed_over,
        Some(false)
    );
    assert!(
        !snapshot.details.fast_venue_available,
        "fallback spot evidence must not be reported as admitted"
    );
    assert!(
        !snapshot.details.reference_current_price_available,
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
        strategy.log_entry_evaluation(replay.evaluation_now_ms, &replay_decision);
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
        .recorded_facts()
        .expect("recorded current evidence must decode")
        .into_iter()
        .filter_map(|event| match event {
            CurrentFact::EntrySkipObservation(skip) => Some(skip),
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
        skip.pricing_blocked_by
            .contains(&crate::bolt_v3_current_evidence::EntryPricingBlockReason::SpotPriceMissing),
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
    let evidence = recording_decision_evidence();
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
            .recorded_facts()
            .expect("recorded current evidence must decode")
            .iter()
            .filter(|event| matches!(event, CurrentFact::EntryOrderIntent(_)))
            .count(),
        2,
        "each shadow entry should still record order-intent evidence"
    );
}

#[test]
fn shadow_policy_entries_do_not_exhaust_live_admission_count_cap() {
    let evidence = recording_decision_evidence();
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
    let admitted_count = evidence
        .recorded_facts()
        .expect("recorded current evidence must decode")
        .into_iter()
        .filter(|event| matches!(event, CurrentFact::AdmittedEntryAdmission(_)))
        .count();
    assert_eq!(
        admitted_count, 2,
        "shadow mode should still record admitted decisions for each would-be entry"
    );
}

#[test]
fn shadow_policy_exit_keeps_pending_exit_between_would_be_exits() {
    let evidence = recording_decision_evidence();
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
            ExitEvaluationTriggerContext::from_local_selection_handler(LocalReceiveMs::new(1_200)),
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
                ExitEvaluationTriggerContext::from_local_selection_handler(LocalReceiveMs::new(
                    1_201,
                )),
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
            .recorded_facts()
            .expect("recorded current evidence must decode")
            .iter()
            .filter(|event| matches!(event, CurrentFact::RiskReducingExitOrderIntent(_)))
            .count(),
        1,
        "latched shadow exit should record one risk-reducing order intent"
    );
    let exit_decisions = evidence
        .recorded_facts()
        .expect("recorded current evidence must decode")
        .into_iter()
        .filter_map(|event| match event {
            CurrentFact::ExitSubmissionDecision(decision) => {
                Some(RecordedExitDecision::Submission(*decision))
            }
            CurrentFact::ExitHoldDecision(decision) => Some(RecordedExitDecision::Hold(*decision)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        exit_decisions.len(),
        2,
        "exit action plus one pending-exit block should be recorded once each"
    );
    assert!(matches!(
        exit_decisions[0],
        RecordedExitDecision::Submission(ref record)
            if record.outcome == ExitSubmissionOutcome::ExitFailClosed
    ));
    let first_details = exit_decisions[0].details();
    assert_eq!(
        first_details.forced_flat_reasons,
        vec![crate::bolt_v3_current_evidence::ForcedFlatReason::Freeze]
    );
    assert_eq!(first_details.exit_eval_now_ms, 1_200);
    assert_eq!(
        first_details.exit_trigger_source,
        ExitTriggerSource::SelectionUpdate
    );
    assert_eq!(first_details.trigger_ts_event_ms, 1_200);
    assert_eq!(first_details.trigger_ts_init_ms, Some(1_200));
    assert_eq!(first_details.rv_surface_id, TEST_SURFACE_ID);
    assert_eq!(first_details.rv_snapshot_as_of_ms, Some(1_200));
    assert!(first_details.rv_snapshot_ready);
    assert_eq!(first_details.rv_snapshot_blockers, Vec::new());
    assert_eq!(first_details.rv_gate_result, RvGateResult::Accepted);
    assert_eq!(first_details.rv_future_dating_delta_ms, None);
    assert!(matches!(
        exit_decisions[1],
        RecordedExitDecision::Hold(ref record) if record.outcome == ExitHoldOutcome::Blocked
    ));
    assert_eq!(
        exit_decisions[1].blocked_reason(),
        Some(ExitBlockedReason::ExitAlreadyPending)
    );
}

#[test]
fn signal_quote_exit_decision_records_future_dated_realized_volatility_gate() {
    let evidence = recording_decision_evidence();
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

    let exit_decisions = recorded_exit_decisions(&evidence);
    assert_eq!(exit_decisions.len(), 1);
    let RecordedExitDecision::Submission(decision) = &exit_decisions[0] else {
        panic!("freeze exit must record a submission decision");
    };
    let details = &decision.details;
    assert_eq!(details.realized_vol, None);
    assert_eq!(details.rv_snapshot_as_of_ms, Some(future_as_of_ms));
    assert!(details.rv_snapshot_ready);
    assert_eq!(details.rv_gate_result, RvGateResult::RejectedFutureDated);
    assert_eq!(details.rv_future_dating_delta_ms, Some(future_delta_ms));
    // Freeze phase forces the position flat, so the recorded exit is a
    // forced-flat Exit: exit_evaluation_at short-circuits on
    // forced_flat_reasons before the RV gate, so the future-dated RV is
    // captured only as a diagnostic (rv_gate_result above), not as the exit
    // cause. RV-driven missing valuation input holds rather than liquidating by
    // default; that path is covered by the pricing / exposure tests.
    assert_eq!(decision.outcome, ExitSubmissionOutcome::ExitFailClosed);
    assert_eq!(
        details.forced_flat_reasons,
        vec![crate::bolt_v3_current_evidence::ForcedFlatReason::Freeze]
    );
    assert_eq!(details.spot_price.as_deref(), Some("3100.25"));
    assert_eq!(
        details.spot_venue_name.as_deref(),
        Some("signal_data_client")
    );
    assert!(details.fast_venue_available);
    assert_eq!(details.reference_current_price.as_deref(), Some("3100.5"));
    assert!(details.reference_current_price_available);
    assert_eq!(details.interval_open.as_deref(), Some("3100"));
    assert_eq!(details.fair_probability_up, None);
    assert_eq!(details.fair_probability_down, None);
    assert_eq!(details.uncertainty_band_probability, None);
    assert_eq!(details.up_fee_bps, None);
    assert_eq!(details.down_fee_bps, None);
    assert!(
        decision.submission.order_side != EvidenceOrderSide::Unspecified,
        "exit decision evidence must preserve the submitted order side"
    );
    assert!(
        !decision.submission.price.is_empty(),
        "exit decision evidence must preserve the submitted order price"
    );
    assert!(
        !decision.submission.quantity.is_empty(),
        "exit decision evidence must preserve the submitted order quantity"
    );
    assert_eq!(details.exit_trigger_source, ExitTriggerSource::SignalQuote);
    assert_eq!(details.trigger_ts_event_ms, exit_eval_now_ms);
    assert_eq!(details.trigger_ts_init_ms, Some(exit_eval_now_ms));
    assert_eq!(details.exit_eval_now_ms, exit_eval_now_ms);
}

#[test]
fn shadow_policy_surfaces_admission_rejection_and_clears_pending_entry() {
    let evidence = recording_decision_evidence();
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
    let rejection_reasons = evidence
        .recorded_facts()
        .expect("recorded current evidence must decode")
        .into_iter()
        .filter_map(|event| match event {
            CurrentFact::RejectedEntryAdmission(admission) => Some(admission.reason),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        rejection_reasons,
        vec![AdmissionRejectionReason::NotionalCapExceeded],
        "a rejected shadow entry must still record the rejected admission decision"
    );
}

#[test]
fn strategy_input_evidence_records_realized_volatility_unknown_source_rejections() {
    let evidence = recording_decision_evidence();
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

    let events = evidence
        .recorded_facts()
        .expect("recorded current evidence must decode");
    let Some(CurrentFact::SubmitLinkedStrategyInputSnapshot(snapshot)) = events.first() else {
        panic!("expected first evidence event to be strategy input; got {events:#?}");
    };
    let StrategyInputRvState::Present { snapshot, .. } = &snapshot.details.realized_volatility
    else {
        panic!("ready RV snapshot must be present");
    };
    assert_eq!(
        snapshot
            .unknown_source_rejections
            .get("<UNKNOWN_SOURCE_ID>"),
        Some(&2)
    );
}

#[test]
fn strategy_input_evidence_accepts_ready_surfaced_zero_realized_volatility() {
    let evidence = recording_decision_evidence();
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

    let events = evidence
        .recorded_facts()
        .expect("recorded current evidence must decode");
    let Some(CurrentFact::SubmitLinkedStrategyInputSnapshot(snapshot)) = events.first() else {
        panic!("expected first evidence event to be strategy input; got {events:#?}");
    };
    let StrategyInputRvState::Present {
        selected_annualized_decimal,
        snapshot,
        ..
    } = &snapshot.details.realized_volatility
    else {
        panic!("ready zero RV snapshot must be present");
    };
    assert_eq!(selected_annualized_decimal.as_deref(), Some("0"));
    assert_eq!(snapshot.annualized_decimal.as_deref(), Some("0"));
}

#[test]
fn blocked_strategy_input_evidence_records_state_transitions_not_ticks() {
    let evidence = recording_decision_evidence();
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

    let events = evidence
        .recorded_facts()
        .expect("recorded current evidence must decode");
    let blocked_snapshots = events
        .iter()
        .filter_map(|event| match event {
            CurrentFact::BlockedStrategyInputObservation(snapshot) => Some(snapshot),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        blocked_snapshots.len(),
        1,
        "identical blocked evaluations emit once, and an RV blocker-list transition no longer \
         emits a second. This producer's registered axis is the gate result paired with \
         watermark presence; the blocker list is a diagnostic the frozen contract excludes \
         from episode identity. The forensic detail survives in the payload of the record \
         that did emit, rather than as one append per transition"
    );
    let rv_blockers =
        |record: &crate::bolt_v3_current_evidence::BlockedStrategyInputObservationFact| {
            match &record.details.realized_volatility {
                StrategyInputRvState::Present { snapshot, .. } => snapshot.blockers.clone(),
                StrategyInputRvState::Absent { .. } => panic!("fixture provides an RV snapshot"),
            }
        };
    assert_eq!(
        rv_blockers(blocked_snapshots[0]),
        vec![crate::bolt_v3_current_evidence::RealizedVolBlockReason::QuorumNotReady]
    );
    let Some(CurrentFact::BlockedStrategyInputObservation(snapshot)) = events.first() else {
        panic!("expected blocked strategy input evidence first; got {events:#?}");
    };
    let StrategyInputRvState::Present {
        selected_annualized_decimal,
        snapshot: rv_snapshot,
        ..
    } = &snapshot.details.realized_volatility
    else {
        panic!("fixture provides an RV snapshot");
    };
    assert_eq!(rv_snapshot.surface_id, TEST_SURFACE_ID);
    assert_eq!(rv_snapshot.as_of_ms, Some(1_200));
    assert_eq!(selected_annualized_decimal, &None);
    assert_eq!(rv_snapshot.annualized_decimal, None);
    assert_eq!(
        rv_snapshot.blockers,
        vec![crate::bolt_v3_current_evidence::RealizedVolBlockReason::QuorumNotReady]
    );
    assert_eq!(
        snapshot.details.pricing_blocked_by,
        vec![crate::bolt_v3_current_evidence::EntryPricingBlockReason::RealizedVolNotReady]
    );
    let entry_skips = events
        .iter()
        .filter_map(|event| match event {
            CurrentFact::EntrySkipObservation(skip) => Some(skip),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        entry_skips.len(),
        1,
        "same blocked interval/reason must emit one entry skip record"
    );
    let skip = entry_skips[0];
    assert_eq!(skip.reason_category, EntrySkipReason::EntryPricingBlocked);
    assert_eq!(
        skip.pricing_blocked_by,
        vec![crate::bolt_v3_current_evidence::EntryPricingBlockReason::RealizedVolNotReady]
    );
    assert_eq!(skip.market_id, strategy.active.market_id);
    // RV not ready: the readiness-gated source path yields no usable RV, so the
    // entry-skip evidence carries no source ts. (The raw as_of_ms is still
    // recorded on the StrategyInput evidence above via the audit path.)
    assert_eq!(skip.realized_vol_source_ts_ms, None);
}

#[test]
fn entry_skip_evidence_records_distinct_pricing_blockers_in_same_interval() {
    let evidence = recording_decision_evidence();
    let submit_admission = submit_admission_with_provider_cap(Decimal::new(1, 2), evidence.clone());
    let mut strategy = ready_to_trade_strategy_with_decision_evidence_and_submit_admission(
        evidence.clone(),
        submit_admission,
    );
    register_test_strategy_with_active_instruments(&mut strategy);

    let mut realized_vol_not_ready = minimal_entry_submission_decision();
    realized_vol_not_ready.evaluation.pricing_blocked_by =
        vec![EntryPricingBlockReason::RealizedVolNotReady];
    let mut spot_missing = minimal_entry_submission_decision();
    spot_missing.evaluation.pricing_blocked_by = vec![EntryPricingBlockReason::SpotPriceMissing];

    strategy
        .record_entry_skip_once(
            1_200,
            &realized_vol_not_ready,
            EntrySkipReason::EntryPricingBlocked,
        )
        .expect("first pricing-blocked skip should record");
    strategy
        .record_entry_skip_once(1_201, &spot_missing, EntrySkipReason::EntryPricingBlocked)
        .expect("distinct pricing blocker in same interval should record");

    let entry_skips = evidence
        .recorded_facts()
        .expect("recorded current evidence must decode")
        .into_iter()
        .filter_map(|event| match event {
            CurrentFact::EntrySkipObservation(skip) => Some(skip),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        entry_skips.len(),
        1,
        "same skip reason with different pricing blockers now dedupes. Pricing blockers are \
         diagnostics, and this producer's registered axis is the skip reason -- keying on the \
         blocker list is what let a flapping blocker append on every tick"
    );
    assert_eq!(entry_skips[0].market_id, strategy.active.market_id);
    assert_eq!(
        entry_skips[0].pricing_blocked_by,
        vec![crate::bolt_v3_current_evidence::EntryPricingBlockReason::RealizedVolNotReady]
    );
}

#[test]
fn entry_skip_dedupe_records_liveness_state_transitions_not_price_ticks() {
    let evidence = recording_decision_evidence();
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
        .record_entry_skip_once(1_200, &spot_missing, EntrySkipReason::EntryPricingBlocked)
        .expect("first spot-missing skip should record");

    strategy.latest_signal_quote = Some(fast_spot("bybit", 3_101.5, 1_201));
    strategy
        .record_entry_skip_once(1_201, &spot_missing, EntrySkipReason::EntryPricingBlocked)
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
        .record_entry_skip_once(1_202, &spot_missing, EntrySkipReason::EntryPricingBlocked)
        .expect("same blocker with a liveness-state transition should record");

    let entry_skips = evidence
        .recorded_facts()
        .expect("recorded current evidence must decode")
        .into_iter()
        .filter_map(|event| match event {
            CurrentFact::EntrySkipObservation(skip) => Some(skip),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        entry_skips.len(),
        1,
        "the same skip reason records once per episode; a liveness-state transition beneath it \
         is a diagnostic and no longer appends. The initial skip is the record that survives, \
         so its payload is the one that must be right"
    );
    assert_eq!(
        entry_skips[0].spot_price, None,
        "the surviving record is the initial skip, taken before spot became available"
    );
    // The liveness transition's own reference price and timestamp are no longer
    // a second record to assert on. `liveness_transition_ts_ms` stays in the
    // fixture because the transition still has to happen for this test to mean
    // anything -- what changed is that it no longer appends.
    let _ = liveness_transition_ts_ms;
}

#[test]
fn entry_skip_dedupe_does_not_record_every_reference_tick_while_blocked() {
    let replay = strategy_input_quote_replay();
    let evidence = recording_decision_evidence();
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
                EntrySkipReason::EntryPricingBlocked,
            )
            .expect("blocked entry skip should record or dedupe without error");
    }

    let entry_skips = evidence
        .recorded_facts()
        .expect("recorded current evidence must decode")
        .into_iter()
        .filter_map(|event| match event {
            CurrentFact::EntrySkipObservation(skip) => Some(skip),
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
    let evidence = recording_decision_evidence();
    let submit_admission = submit_admission_with_provider_cap(Decimal::new(1, 2), evidence.clone());
    let mut strategy = ready_to_trade_strategy_with_decision_evidence_and_submit_admission(
        evidence.clone(),
        submit_admission,
    );
    register_test_strategy_with_active_instruments(&mut strategy);
    strategy.active.interval_end_ms = Some(301_999);
    strategy
        .pricing
        .seed_ready_realized_vol(Some(TEST_SOURCE_ID.to_string()), 1.5, 2_000);

    strategy
        .try_submit_entry_order(2_000)
        .expect_err("submit admission should reject after evidence capture");

    let events = evidence
        .recorded_facts()
        .expect("recorded current evidence must decode");
    let Some(CurrentFact::SubmitLinkedStrategyInputSnapshot(snapshot)) = events.first() else {
        panic!("expected first evidence event to be strategy input; got {events:#?}");
    };

    assert_eq!(snapshot.details.seconds_to_market_end, 299);
    assert_eq!(snapshot.details.market_selection_timestamp_ms, Some(1_000));
    assert_eq!(
        snapshot.details.polymarket_market_start_timestamp_ms,
        Some(1_000)
    );
    assert_eq!(
        snapshot.details.polymarket_market_end_timestamp_ms,
        Some(301_999),
        "market end must bind to selected expiration without seconds rounding"
    );
}

#[test]
fn strategy_input_evidence_records_next_market_selection_outcome() {
    let evidence = recording_decision_evidence();
    let submit_admission = submit_admission_with_provider_cap(Decimal::new(1, 2), evidence.clone());
    let mut strategy = ready_to_trade_strategy_with_decision_evidence_and_submit_admission(
        evidence.clone(),
        submit_admission,
    );
    register_test_strategy_with_active_instruments(&mut strategy);
    strategy.active.market_selection_outcome = MarketSelectionOutcome::Next;
    strategy
        .pricing
        .seed_ready_realized_vol(Some(TEST_SOURCE_ID.to_string()), 1.5, 2_000);

    strategy
        .try_submit_entry_order(2_000)
        .expect_err("submit admission should reject after evidence capture");

    let events = evidence
        .recorded_facts()
        .expect("recorded current evidence must decode");
    let Some(CurrentFact::SubmitLinkedStrategyInputSnapshot(snapshot)) = events.first() else {
        panic!("expected first evidence event to be strategy input; got {events:#?}");
    };

    assert_eq!(
        snapshot.details.market_selection_outcome,
        crate::bolt_v3_current_evidence::StrategyInputMarketSelectionOutcome::Next
    );
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
            gate_result: RvGateResult::MissingSnapshot,
            receive_watermark_ms: None,
            realized_vol: None,
            source_venue: None,
            source_ts_ms: None,
            evidence: RealizedVolatilityEvidenceFields {
                surface_id: String::new(),
                as_of_ms: None,
                annualized_decimal: None,
                measured_annualized_decimal: None,
                noise_robust_annualized_decimal: None,
                continuous_annualized_decimal: None,
                jump_annualized_decimal: None,
                forecast_annualized_decimal: None,
                pricing_component: None,
                seconds_per_annum: String::new(),
                aggregation: None,
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
        planned_fill_legs: Vec::new(),
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
            realized_volatility_receipt: ExitRealizedVolatilityGateReceipt {
                gate_result: RvGateResult::MissingSnapshot,
                surface_id: TEST_SURFACE_ID.to_string(),
                max_source_age_ms: 500,
                evaluation_receive_ms: Some(LocalReceiveMs::new(1_200)),
                snapshot_as_of_ms: None,
                snapshot_receive_watermark_ms: None,
                snapshot_ready: false,
                snapshot_has_ready_realized_vol: false,
                realized_vol: None,
                realized_vol_source_venue: None,
                realized_vol_source_ts_ms: None,
                raw_snapshot_blockers: Vec::new(),
                source_diagnostics: Vec::new(),
                snapshot_as_of_minus_trigger_event_ms: None,
                fair_probability_up: None,
                fair_probability_down: None,
                uncertainty_band_probability: None,
            },
            position_outcome_side: None,
            forced_flat_reasons: vec![],
            hold_ev_bps: None,
            exit_ev_bps: None,
            exit_decision: Some(ExitDecision::Hold),
            blocked_reason: Some(EvidenceExitBlockedReason::ExitHold),
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
        blocked_reason: Some(EvidenceExitBlockedReason::ExitHold),
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
        failing_decision_evidence(),
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
fn terminal_close_reclaims_exit_decision_for_reused_position_identity() {
    let (mut strategy, evidence) = exit_evidence_strategy_with_open_position();
    let reused_exposure = strategy.exposure.clone();
    let (instrument_id, position_id) = match &reused_exposure {
        ExposureState::Managed(position) => (position.instrument_id, position.position_id),
        other => panic!("exit fixture must begin managed, got {other:?}"),
    };
    let trigger = ExitEvaluationTriggerContext::unknown(1_200);
    let decision = minimal_exit_submission_decision();

    strategy
        .record_exit_decision_once(1_200, trigger, &decision)
        .expect("the first position decision should record");
    strategy
        .record_exit_decision_once(1_201, trigger, &decision)
        .expect("an adjacent duplicate should be a successful no-op");
    assert_eq!(
        recorded_exit_decisions(&evidence).len(),
        1,
        "the live position should suppress its adjacent duplicate"
    );

    close_nt_position(&mut strategy, position_id);
    strategy.on_position_closed(position_closed_event(instrument_id, position_id));
    assert!(
        strategy.last_recorded_exit_decision.is_none(),
        "terminal close must retire the dead position's adjacent-repeat key"
    );

    // NT netting reuses `{instrument}-{strategy}` as PositionId. Recreate the
    // same logical position identity and prove its first decision is not
    // suppressed by the predecessor's last one.
    strategy.exposure = reused_exposure;
    strategy
        .record_exit_decision_once(1_202, trigger, &decision)
        .expect("the reused position identity should record its first decision");
    assert_eq!(
        recorded_exit_decisions(&evidence).len(),
        2,
        "a new position under the reused NT identity must emit its first exit decision"
    );
}

/// Build a ready-to-trade strategy with one open managed position and a recording
/// evidence writer, for #885 exit-evaluation evidence tests. Returns the strategy
/// and the writer (the caller drives exits and reads back `events()`).
fn exit_evidence_strategy_with_open_position() -> (
    BinaryOracleEdgeTaker,
    Arc<crate::bolt_v3_current_evidence::DecisionEvidenceRecorder>,
) {
    let evidence = recording_decision_evidence();
    let strategy = exit_evidence_strategy_with_open_position_using_writer(evidence.clone());
    (strategy, evidence)
}

/// Build the open-position exit-evidence fixture against an arbitrary decision
/// evidence writer. Shared by the recording-writer tests and the failing-writer
/// swallow test so the open-position setup lives in ONE place.
fn exit_evidence_strategy_with_open_position_using_writer(
    evidence: Arc<crate::bolt_v3_current_evidence::DecisionEvidenceRecorder>,
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
    evidence: &crate::bolt_v3_current_evidence::DecisionEvidenceRecorder,
) -> Vec<crate::bolt_v3_current_evidence::ExitEvaluationFact> {
    evidence
        .recorded_facts()
        .expect("recorded current evidence must decode")
        .into_iter()
        .filter_map(|event| match event {
            CurrentFact::ExitEvaluation(evidence) => Some(*evidence),
            _ => None,
        })
        .collect()
}

/// Collect every recorded exit-decision evidence record, in order.
#[derive(Debug, Clone)]
enum RecordedExitDecision {
    Submission(crate::bolt_v3_current_evidence::ExitSubmissionDecisionFact),
    Hold(crate::bolt_v3_current_evidence::ExitHoldDecisionFact),
}

impl RecordedExitDecision {
    fn details(&self) -> &crate::bolt_v3_current_evidence::ExitDecisionDetails {
        match self {
            Self::Submission(record) => &record.details,
            Self::Hold(record) => &record.details,
        }
    }

    fn blocked_reason(&self) -> Option<crate::bolt_v3_current_evidence::ExitBlockedReason> {
        match self {
            Self::Submission(_) => None,
            Self::Hold(record) => record.blocked_reason,
        }
    }
}

fn recorded_exit_decisions(
    evidence: &crate::bolt_v3_current_evidence::DecisionEvidenceRecorder,
) -> Vec<RecordedExitDecision> {
    evidence
        .recorded_facts()
        .expect("recorded current evidence must decode")
        .into_iter()
        .filter_map(|event| match event {
            CurrentFact::ExitSubmissionDecision(record) => {
                Some(RecordedExitDecision::Submission(*record))
            }
            CurrentFact::ExitHoldDecision(record) => Some(RecordedExitDecision::Hold(*record)),
            _ => None,
        })
        .collect()
}

#[test]
fn signal_quote_exit_uses_pinned_quote_receive_stamp_without_fallback() {
    let (mut strategy, evidence) = exit_evidence_strategy_with_open_position();
    let lifecycle_now_ms: u64 = 1_200;
    let position_interval_end_ms = strategy
        .managed_position()
        .and_then(|managed| managed.position.lifecycle.interval_end_ms())
        .expect("exit fixture must retain the position's interval end");
    let signal_event_ms = position_interval_end_ms.saturating_add(1);
    let signal_receive_ms = 1_220;
    let (_cache, clock) = register_test_strategy_with_clock(&mut strategy);
    clock.borrow_mut().set_time(UnixNanos::from(
        lifecycle_now_ms.saturating_mul(NANOS_PER_MILLI_U64),
    ));
    strategy.pricing.seed_ready_realized_vol(
        Some("<SOURCE_ID>".to_string()),
        1.5,
        signal_receive_ms,
    );
    let signal_instrument_id = strategy
        .config
        .signal_instrument_id
        .as_deref()
        .expect("exit fixture must configure a signal instrument")
        .to_string();
    let signal_price = strategy
        .pricing
        .spot_price()
        .expect("exit fixture must expose a signal price");

    strategy
        .on_quote(&quote_tick_with_stamps(
            &signal_instrument_id,
            signal_price - 0.01,
            signal_price + 0.01,
            signal_event_ms,
            signal_receive_ms,
        ))
        .expect("unequal-stamped signal quote must reach exit evaluation");

    let records = recorded_exit_evaluations(&evidence);
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].exit_eval_now_ms, lifecycle_now_ms as i64);
    assert_eq!(records[0].trigger_ts_event_ms, Some(signal_event_ms as i64));
    assert_eq!(
        records[0].trigger_ts_init_ms,
        Some(signal_receive_ms as i64)
    );
    assert_eq!(
        records[0].rv_gate_result,
        crate::bolt_v3_current_evidence::RvGateResult::Accepted,
        "signal RV evaluation must use QuoteTick.ts_init, not its venue event stamp or strategy clock"
    );
    assert!(matches!(
        records[0].decision,
        crate::bolt_v3_current_evidence::ExitEvaluationDecision::Submission { .. }
    ));
}

#[test]
fn invalid_signal_quote_exit_preserves_lifecycle_event_and_receive_domains() {
    let (mut strategy, evidence) = exit_evidence_strategy_with_open_position();
    let lifecycle_now_ms: u64 = 1_200;
    let position_interval_end_ms = strategy
        .managed_position()
        .and_then(|managed| managed.position.lifecycle.interval_end_ms())
        .expect("exit fixture must retain the position's interval end");
    let signal_event_ms = position_interval_end_ms.saturating_add(1);
    let signal_receive_ms = 1_220;
    assert_ne!(lifecycle_now_ms, signal_event_ms);
    assert_ne!(lifecycle_now_ms, signal_receive_ms);
    assert_ne!(signal_event_ms, signal_receive_ms);
    let (_cache, clock) = register_test_strategy_with_clock(&mut strategy);
    clock.borrow_mut().set_time(UnixNanos::from(
        lifecycle_now_ms.saturating_mul(NANOS_PER_MILLI_U64),
    ));
    strategy.pricing.seed_ready_realized_vol(
        Some("<SOURCE_ID>".to_string()),
        1.5,
        signal_receive_ms,
    );
    let signal_instrument_id = strategy
        .config
        .signal_instrument_id
        .as_deref()
        .expect("exit fixture must configure a signal instrument")
        .to_string();

    strategy
        .on_quote(&invalid_quote_tick_with_stamps(
            &signal_instrument_id,
            signal_event_ms,
            signal_receive_ms,
        ))
        .expect("invalid unequal-stamped signal quote must reach exit evaluation");

    let records = recorded_exit_evaluations(&evidence);
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].exit_eval_now_ms, lifecycle_now_ms as i64);
    assert_eq!(records[0].trigger_ts_event_ms, Some(signal_event_ms as i64));
    assert_eq!(
        records[0].trigger_ts_init_ms,
        Some(signal_receive_ms as i64)
    );
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
                crate::bolt_v3_current_evidence::ExitTriggerSource::SignalQuote,
                1_200,
                Some(1_220),
            ),
        )
        .expect("control exit evaluation should not error with a ready realized-vol surface");

    let failing_evidence = failing_observation_evidence();
    let mut failing_strategy =
        exit_evidence_strategy_with_open_position_using_writer(failing_evidence.clone());
    failing_strategy
        .pricing
        .seed_ready_realized_vol(Some("<SOURCE_ID>".to_string()), 1.5, 1_200);
    let failing_result = failing_strategy
        .try_submit_exit_order_for_trigger(
            1_200,
            ExitEvaluationTriggerContext::new(
                crate::bolt_v3_current_evidence::ExitTriggerSource::SignalQuote,
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
        failing_evidence.attempts_for(
            crate::bolt_v3_current_evidence::generated_contract::KnownPurpose::ExitEvaluation
        ),
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
        failing_decision_evidence(),
    );
    let strategy_id = unique_log_capture_strategy_id("entry");
    strategy.config.strategy_id = strategy_id.clone();

    let decision = minimal_entry_submission_decision();
    let result = with_captured_error_log(
        "binary_oracle_edge_taker entry skip evidence write failed",
        &strategy_id,
        || strategy.record_entry_skip_once(1_000, &decision, EntrySkipReason::NoSideSelected),
    );

    assert!(
        result.is_ok(),
        "an entry-skip evidence write failure must not abort the strategy callback: {result:?}"
    );
    assert!(
        !strategy
            .record_entry_skip_once(1_000, &decision, EntrySkipReason::NoSideSelected)
            .expect("a second identical skip must not abort the callback either"),
        "the episode must stay marked through a failed write, so the lost record is not \
         retried in a tight loop"
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
                crate::bolt_v3_current_evidence::ExitTriggerSource::SignalQuote,
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
        record.details().spot_price,
        None,
        "the position-coupled exit spot price should be absent for a market mismatch"
    );
    assert!(
        record.details().fast_venue_available,
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
                crate::bolt_v3_current_evidence::ExitTriggerSource::SignalQuote,
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
        crate::bolt_v3_current_evidence::RvGateResult::Accepted,
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
        crate::bolt_v3_current_evidence::ExitTriggerSource::SignalQuote,
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
    assert_eq!(record.up_fee_bps, None);
    assert_eq!(record.down_fee_bps, None);
}

#[test]
fn exit_evaluation_policy_exit_is_recordable_before_submission_linkage_exists() {
    let (mut strategy, evidence) = exit_evidence_strategy_with_open_position();
    strategy
        .pricing
        .seed_ready_realized_vol(Some("<SOURCE_ID>".to_string()), 1.5, 1_200);
    let trigger = ExitEvaluationTriggerContext::new(
        crate::bolt_v3_current_evidence::ExitTriggerSource::SignalQuote,
        1_200,
        Some(1_200),
    );
    let decision = strategy.exit_submission_decision_for_trigger_at(1_200, trigger);
    assert!(matches!(
        decision.evaluation.exit_decision,
        Some(ExitDecision::Exit)
    ));
    assert!(
        decision.client_order_id.is_none(),
        "evaluation precedes order construction and cannot require submit linkage"
    );

    strategy.record_exit_evaluation_evidence(1_200, &decision, trigger, false);

    let records = recorded_exit_evaluations(&evidence);
    assert_eq!(
        records.len(),
        1,
        "a diagnostic exit-policy result must be recordable before submission linkage exists"
    );
    assert!(matches!(
        records[0].decision,
        crate::bolt_v3_current_evidence::ExitEvaluationDecision::Submission { .. }
    ));
}

#[test]
fn exit_evaluation_evidence_reports_fast_venue_when_position_spot_is_absent() {
    let (mut strategy, evidence) = exit_evidence_strategy_with_open_position();
    strategy.active.market_id = Some("different-active-market".to_string());

    strategy.record_exit_evaluation_evidence(
        1_200,
        &minimal_exit_submission_decision(),
        ExitEvaluationTriggerContext::new(
            crate::bolt_v3_current_evidence::ExitTriggerSource::SignalQuote,
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
            crate::bolt_v3_current_evidence::ExitTriggerSource::SignalQuote,
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
    assert!(matches!(
        record.decision,
        crate::bolt_v3_current_evidence::ExitEvaluationDecision::Hold { .. }
    ));
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
                crate::bolt_v3_current_evidence::ExitTriggerSource::BookDelta,
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
        crate::bolt_v3_current_evidence::RvGateResult::RejectedFutureDated,
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
        crate::bolt_v3_current_evidence::RvGateResult::Accepted,
        "local triggers must use their receive-domain evaluation timestamp"
    );
    assert_eq!(record.rv_as_of_ms, Some(1_200));
    assert_eq!(record.rv_as_of_minus_now_ms, None);
}

#[test]
fn diagnostic_exit_evaluation_holds_when_receive_time_is_structurally_absent() {
    let (mut strategy, evidence) = exit_evidence_strategy_with_open_position();
    strategy.active.phase = SelectionPhase::Active;
    strategy
        .pricing
        .seed_ready_realized_vol(Some("<SOURCE_ID>".to_string()), 1.5, 1_200);

    strategy
        .try_submit_exit_order_for_trigger(
            1_200,
            ExitEvaluationTriggerContext::diagnostic_missing(
                crate::bolt_v3_current_evidence::ExitTriggerSource::Other,
                1_200,
            ),
        )
        .expect("missing receive context should fail closed without an error");

    let records = recorded_exit_evaluations(&evidence);
    assert_eq!(records.len(), 1);
    let record = &records[0];
    assert_eq!(
        record.rv_gate_result,
        crate::bolt_v3_current_evidence::RvGateResult::MissingEvaluationEventTime
    );
    assert_eq!(
        record.decision,
        crate::bolt_v3_current_evidence::ExitEvaluationDecision::Hold {
            outcome: ExitHoldOutcome::Blocked,
            blocked_reason: Some(ExitBlockedReason::ExitHold),
        },
        "missing receive-domain input must record the explicit hold block, never liquidate by default"
    );
    assert_eq!(record.fair_probability_up, None);
    assert_eq!(record.fair_probability_down, None);
    assert_eq!(record.uncertainty_band_probability, None);
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
            crate::bolt_v3_current_evidence::ExitTriggerSource::BookDelta,
            1_200,
            Some(1_210),
        ),
        ExitEvaluationTriggerContext::new(
            crate::bolt_v3_current_evidence::ExitTriggerSource::SelectionUpdate,
            1_220,
            Some(1_220),
        ),
        ExitEvaluationTriggerContext::new(
            crate::bolt_v3_current_evidence::ExitTriggerSource::SignalQuote,
            1_200,
            Some(1_230),
        ),
        ExitEvaluationTriggerContext::new(
            crate::bolt_v3_current_evidence::ExitTriggerSource::SelectionUpdate,
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
                crate::bolt_v3_current_evidence::ExitTriggerSource::BookDelta,
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
fn rv_clock_domain_amendment_exit_decision_and_evidence_stay_stable_across_triggers() {
    let (mut strategy, evidence) = exit_evidence_strategy_with_open_position();
    strategy
        .pricing
        .seed_ready_realized_vol(Some("<SOURCE_ID>".to_string()), 1.5, 1_200);

    let trigger_contexts = [
        ExitEvaluationTriggerContext::new(
            crate::bolt_v3_current_evidence::ExitTriggerSource::BookDelta,
            1_199,
            Some(1_210),
        ),
        ExitEvaluationTriggerContext::new(
            crate::bolt_v3_current_evidence::ExitTriggerSource::SignalQuote,
            1_201,
            Some(1_220),
        ),
        ExitEvaluationTriggerContext::new(
            crate::bolt_v3_current_evidence::ExitTriggerSource::SelectionUpdate,
            1_230,
            Some(1_230),
        ),
        ExitEvaluationTriggerContext::new(
            crate::bolt_v3_current_evidence::ExitTriggerSource::BookDelta,
            1_201,
            Some(1_240),
        ),
    ];

    let mut expected_outcome = None;
    for trigger_context in trigger_contexts {
        let decision = strategy.exit_submission_decision_for_trigger_at(1_240, trigger_context);
        let outcome = (
            decision.evaluation.exit_decision,
            decision.blocked_reason,
            decision.evaluation.hold_ev_bps,
        );
        if let Some(expected) = expected_outcome {
            assert_eq!(
                outcome, expected,
                "trigger clock/source changed the exit decision"
            );
        } else {
            expected_outcome = Some(outcome);
        }
        strategy.record_exit_evaluation_evidence(1_240, &decision, trigger_context, false);
    }

    let records = recorded_exit_evaluations(&evidence);
    assert_eq!(
        records.len(),
        1,
        "one unchanged receive-domain exit outcome must produce one evidence transition"
    );
    assert_eq!(
        records[0].rv_gate_result,
        crate::bolt_v3_current_evidence::RvGateResult::Accepted
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
                crate::bolt_v3_current_evidence::ExitTriggerSource::SignalQuote,
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
                        crate::bolt_v3_current_evidence::ExitTriggerSource::SignalQuote,
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

const RV_RECEIPT_SNAPSHOT_AS_OF_MS: u64 = 1_350;
const RV_RECEIPT_WATERMARK_MS: u64 = 1_180;
const RV_RECEIPT_TRIGGER_EVENT_MS: u64 = 1_100;
const RV_RECEIPT_TRIGGER_RECEIVE_MS: u64 = 1_220;
const RV_RECEIPT_LIFECYCLE_NOW_MS: u64 = 1_260;

fn rv_clock_domain_amendment_set_snapshot_times(
    strategy: &mut BinaryOracleEdgeTaker,
    as_of_ms: u64,
    receive_watermark_ms: Option<u64>,
) {
    strategy
        .pricing
        .seed_ready_realized_vol(Some(TEST_SOURCE_ID.to_string()), 1.5, 1_200);
    let mut snapshot = strategy
        .pricing
        .latest_realized_vol_snapshot_for_surface(TEST_SURFACE_ID)
        .expect("RV fixture should contain a seeded snapshot")
        .clone();
    snapshot.as_of_ms = as_of_ms;
    snapshot.latest_accepted_receive_ms = receive_watermark_ms.map(LocalReceiveMs::new);
    strategy.pricing.clear_latest_realized_vol_snapshot();
    strategy.pricing.observe_realized_vol_snapshot(snapshot);
}

fn rv_clock_domain_amendment_install_raw_ready_blocked_snapshot(
    strategy: &mut BinaryOracleEdgeTaker,
    as_of_ms: u64,
    receive_watermark_ms: Option<u64>,
) {
    rv_clock_domain_amendment_set_snapshot_times(strategy, as_of_ms, receive_watermark_ms);
    let mut snapshot = strategy
        .pricing
        .latest_realized_vol_snapshot_for_surface(TEST_SURFACE_ID)
        .expect("RV fixture should contain a seeded snapshot")
        .clone();
    snapshot.ready = true;
    snapshot.blocked_reasons =
        crate::bolt_v3_realized_volatility::RealizedVolBlockReason::ALL.to_vec();
    snapshot.source_diagnostics = vec![
        crate::bolt_v3_realized_volatility::RealizedVolSourceDiagnostic {
            source_id: TEST_SOURCE_ID.to_string(),
            source_class: crate::bolt_v3_realized_volatility::RealizedVolSourceClass::SpotQuote,
            sample_kind: crate::bolt_v3_realized_volatility::RealizedVolSampleKind::Midpoint,
            enabled: true,
            counts_toward_quorum: true,
            status: crate::bolt_v3_realized_volatility::RealizedVolSourceStatus::Blocked,
            annualized_realized_vol_decimal: Some(1.5),
            measured_annualized_realized_vol_decimal: Some(1.5),
            noise_robust_annualized_realized_vol_decimal: Some(1.5),
            continuous_annualized_realized_vol_decimal: Some(1.5),
            jump_annualized_realized_vol_decimal: Some(0.0),
            first_sample_ts_ms: Some(700),
            last_sample_ts_ms: Some(1_200),
            raw_sample_count: 2,
            grid_sample_count: 2,
            coverage_ratio: 1.0,
            max_inter_sample_gap_ms: Some(500),
            last_rejected_reason: None,
            last_rejected_event_ts_ms: None,
            last_rejected_recv_ts_ms: None,
            rejection_counters: BTreeMap::new(),
            block_reason: Some(
                crate::bolt_v3_realized_volatility::RealizedVolBlockReason::QuorumNotReady,
            ),
        },
    ];
    snapshot
        .unknown_source_rejections
        .insert("<UNKNOWN_SOURCE>".to_string(), 2);
    snapshot.config_fingerprint = "original-fingerprint".to_string();
    strategy.pricing.observe_realized_vol_snapshot(snapshot);
}

#[test]
fn rv_clock_domain_amendment_exit_records_share_the_captured_receipt() {
    let (mut strategy, evidence) = exit_evidence_strategy_with_open_position();
    rv_clock_domain_amendment_set_snapshot_times(
        &mut strategy,
        RV_RECEIPT_SNAPSHOT_AS_OF_MS,
        Some(RV_RECEIPT_WATERMARK_MS),
    );
    let trigger = ExitEvaluationTriggerContext::new(
        crate::bolt_v3_current_evidence::ExitTriggerSource::SignalQuote,
        RV_RECEIPT_TRIGGER_EVENT_MS,
        Some(RV_RECEIPT_TRIGGER_RECEIVE_MS),
    );
    let decision =
        strategy.exit_submission_decision_for_trigger_at(RV_RECEIPT_LIFECYCLE_NOW_MS, trigger);
    strategy
        .record_exit_decision_once(RV_RECEIPT_LIFECYCLE_NOW_MS, trigger, &decision)
        .expect("exit decision evidence should record");
    strategy.record_exit_evaluation_evidence(
        RV_RECEIPT_LIFECYCLE_NOW_MS,
        &decision,
        trigger,
        false,
    );

    let decisions = recorded_exit_decisions(&evidence);
    let evaluations = recorded_exit_evaluations(&evidence);
    assert_eq!(decisions.len(), 1);
    assert_eq!(evaluations.len(), 1);
    let decision = decisions[0].details();
    let evaluation = &evaluations[0];
    assert_eq!(decision.exit_eval_now_ms, RV_RECEIPT_LIFECYCLE_NOW_MS);
    assert_eq!(
        evaluation.exit_eval_now_ms,
        RV_RECEIPT_LIFECYCLE_NOW_MS as i64
    );
    assert_eq!(decision.trigger_ts_event_ms, RV_RECEIPT_TRIGGER_EVENT_MS);
    assert_eq!(
        evaluation.trigger_ts_event_ms,
        Some(RV_RECEIPT_TRIGGER_EVENT_MS as i64)
    );
    assert_eq!(
        decision.trigger_ts_init_ms,
        Some(RV_RECEIPT_TRIGGER_RECEIVE_MS)
    );
    assert_eq!(
        evaluation.trigger_ts_init_ms,
        Some(RV_RECEIPT_TRIGGER_RECEIVE_MS as i64)
    );
    assert_eq!(
        decision.rv_snapshot_as_of_ms,
        Some(RV_RECEIPT_SNAPSHOT_AS_OF_MS)
    );
    assert_eq!(
        evaluation.rv_as_of_ms,
        Some(RV_RECEIPT_SNAPSHOT_AS_OF_MS as i64)
    );
    assert_eq!(decision.rv_gate_result, evaluation.rv_gate_result);
    assert_eq!(decision.rv_gate_result, RvGateResult::Accepted);
    assert_eq!(
        decision.rv_snapshot_receive_watermark_ms,
        Some(RV_RECEIPT_WATERMARK_MS)
    );
    assert_eq!(
        evaluation.rv_snapshot_receive_watermark_ms,
        Some(RV_RECEIPT_WATERMARK_MS as i64)
    );
    assert_eq!(decision.rv_max_source_age_ms, Some(500));
    assert_eq!(evaluation.rv_max_source_age_ms, Some(500));
    assert_eq!(decision.rv_snapshot_has_ready_realized_vol, Some(true));
    assert_eq!(decision.realized_vol.as_deref(), Some("1.5"));
    assert!(evaluation.rv_ready);
    assert_eq!(decision.rv_future_dating_delta_ms, Some(250));
    assert_eq!(evaluation.rv_as_of_minus_now_ms, Some(250));
}

#[test]
fn rv_clock_domain_amendment_exit_receipt_is_retained_across_submission_shapes() {
    let (mut strategy, evidence) = exit_evidence_strategy_with_open_position();
    rv_clock_domain_amendment_set_snapshot_times(
        &mut strategy,
        RV_RECEIPT_SNAPSHOT_AS_OF_MS,
        Some(RV_RECEIPT_WATERMARK_MS),
    );
    let trigger = ExitEvaluationTriggerContext::new(
        crate::bolt_v3_current_evidence::ExitTriggerSource::SignalQuote,
        RV_RECEIPT_TRIGGER_EVENT_MS,
        Some(RV_RECEIPT_TRIGGER_RECEIVE_MS),
    );
    let mut base = strategy.exit_evaluation_for_trigger_at(RV_RECEIPT_LIFECYCLE_NOW_MS, trigger);
    base.forced_flat_reasons.clear();
    base.exit_decision = Some(ExitDecision::Exit);
    base.blocked_reason = None;

    // `ExitEvaluation` requires a receipt at construction, which structurally enforces
    // capture before any production return. This fixture pins transfer and durable
    // projection across the current constructed submission outcomes; it is not a
    // dynamic branch-coverage harness.
    let mut decisions = Vec::new();
    for blocked_reason in [
        EvidenceExitBlockedReason::NoOpenPosition,
        EvidenceExitBlockedReason::ExitAlreadyPending,
        EvidenceExitBlockedReason::PositionIntervalEnded,
        EvidenceExitBlockedReason::PositionIntervalUnknown,
        EvidenceExitBlockedReason::EntryOrderStillWorking,
    ] {
        let mut evaluation = base.clone();
        evaluation.forced_flat_reasons.clear();
        evaluation.exit_decision = None;
        evaluation.blocked_reason = Some(blocked_reason);
        decisions.push(strategy.exit_submission_decision_from_evaluation(evaluation));
    }

    let mut unavailable = base.clone();
    unavailable.forced_flat_reasons.clear();
    unavailable.exit_decision = None;
    unavailable.blocked_reason = None;
    decisions.push(strategy.exit_submission_decision_from_evaluation(unavailable));

    let mut hold = base.clone();
    hold.forced_flat_reasons.clear();
    hold.exit_decision = Some(ExitDecision::Hold);
    hold.blocked_reason = None;
    decisions.push(strategy.exit_submission_decision_from_evaluation(hold));

    let saved_exposure = strategy.exposure.clone();
    strategy.exposure = ExposureState::Flat;
    decisions.push(strategy.exit_submission_decision_from_evaluation(base.clone()));
    strategy.exposure = saved_exposure;

    let saved_exit_order = strategy.config.exit_order.clone();
    strategy.config.exit_order.side = "not-an-order-side".to_string();
    decisions.push(strategy.exit_submission_decision_from_evaluation(base.clone()));
    strategy.config.exit_order = saved_exit_order.clone();

    strategy.config.exit_order.is_quote_quantity = true;
    decisions.push(strategy.exit_submission_decision_from_evaluation(base.clone()));
    strategy.config.exit_order = saved_exit_order.clone();

    strategy.config.exit_order.position_side = "short".to_string();
    decisions.push(strategy.exit_submission_decision_from_evaluation(base.clone()));
    strategy.config.exit_order = saved_exit_order;

    for decision in &decisions {
        let receipt = &decision.evaluation.realized_volatility_receipt;
        assert_eq!(receipt.gate_result, RvGateResult::Accepted);
        assert_eq!(
            receipt.snapshot_as_of_ms,
            Some(RV_RECEIPT_SNAPSHOT_AS_OF_MS)
        );
        assert_eq!(
            receipt.evaluation_receive_ms.map(LocalReceiveMs::value),
            Some(RV_RECEIPT_TRIGGER_RECEIVE_MS)
        );
        assert_eq!(
            receipt
                .snapshot_receive_watermark_ms
                .map(LocalReceiveMs::value),
            Some(RV_RECEIPT_WATERMARK_MS),
            "every constructed submission shape must retain the captured watermark"
        );
        assert_eq!(receipt.max_source_age_ms, 500);
        assert!(receipt.snapshot_has_ready_realized_vol);
    }

    assert_eq!(
        decisions.len(),
        11,
        "the fixture must retain every constructible submission shape"
    );
    let intentionally_non_recordable = &decisions[0];
    assert_eq!(
        intentionally_non_recordable.blocked_reason,
        Some(EvidenceExitBlockedReason::NoOpenPosition)
    );
    assert!(intentionally_non_recordable.forced_flat_reasons.is_empty());
    assert_eq!(
        decisions
            .iter()
            .filter(|decision| {
                decision.blocked_reason == Some(EvidenceExitBlockedReason::NoOpenPosition)
                    && decision.forced_flat_reasons.is_empty()
            })
            .count(),
        1,
        "no_open_position without forced-flat reasons must be the sole non-recordable decision"
    );

    for (index, decision) in decisions.iter().enumerate() {
        strategy.last_recorded_exit_decision = None;
        strategy
            .record_exit_decision_once(
                RV_RECEIPT_LIFECYCLE_NOW_MS + index as u64,
                trigger,
                decision,
            )
            .expect("each submission-shape recording attempt should preserve writer semantics");
    }

    let records = recorded_exit_decisions(&evidence);
    let persisted_blocked_reasons = records
        .iter()
        .map(|record| {
            record
                .blocked_reason()
                .expect("every recordable submission shape must persist its blocked reason")
        })
        .collect::<Vec<_>>();
    let expected_blocked_reasons = vec![
        ExitBlockedReason::ExitAlreadyPending,
        ExitBlockedReason::PositionIntervalEnded,
        ExitBlockedReason::PositionIntervalUnknown,
        ExitBlockedReason::EntryOrderStillWorking,
        ExitBlockedReason::ExitDecisionUnavailable,
        ExitBlockedReason::ExitHold,
        ExitBlockedReason::OpenPositionMissing,
        ExitBlockedReason::ExitOrderConfigInvalid,
        ExitBlockedReason::ExitQuoteQuantityUnsupported,
        ExitBlockedReason::ExitPriceMissing,
    ];
    assert_eq!(persisted_blocked_reasons, expected_blocked_reasons);
    for (index, record) in records.into_iter().enumerate() {
        let details = record.details();
        assert_eq!(
            details.exit_eval_now_ms,
            RV_RECEIPT_LIFECYCLE_NOW_MS + index as u64 + 1
        );
        assert_eq!(details.trigger_ts_event_ms, RV_RECEIPT_TRIGGER_EVENT_MS);
        assert_eq!(
            details.rv_snapshot_as_of_ms,
            Some(RV_RECEIPT_SNAPSHOT_AS_OF_MS)
        );
        assert_eq!(
            details.trigger_ts_init_ms,
            Some(RV_RECEIPT_TRIGGER_RECEIVE_MS)
        );
        assert_eq!(
            details.rv_snapshot_receive_watermark_ms,
            Some(RV_RECEIPT_WATERMARK_MS),
            "every persisted submission shape must retain the captured watermark"
        );
        assert_eq!(details.rv_max_source_age_ms, Some(500));
        assert_eq!(details.rv_snapshot_has_ready_realized_vol, Some(true));
    }
}

#[test]
fn rv_clock_domain_amendment_exit_receipt_is_fully_immutable_after_snapshot_replacement() {
    let (mut strategy, evidence) = exit_evidence_strategy_with_open_position();
    rv_clock_domain_amendment_install_raw_ready_blocked_snapshot(
        &mut strategy,
        RV_RECEIPT_SNAPSHOT_AS_OF_MS,
        Some(RV_RECEIPT_WATERMARK_MS),
    );
    let trigger = ExitEvaluationTriggerContext::new(
        crate::bolt_v3_current_evidence::ExitTriggerSource::SignalQuote,
        RV_RECEIPT_TRIGGER_EVENT_MS,
        Some(RV_RECEIPT_TRIGGER_RECEIVE_MS),
    );
    let decision =
        strategy.exit_submission_decision_for_trigger_at(RV_RECEIPT_LIFECYCLE_NOW_MS, trigger);
    strategy
        .record_exit_decision_once(RV_RECEIPT_LIFECYCLE_NOW_MS, trigger, &decision)
        .expect("original exit decision evidence should record");
    strategy.record_exit_evaluation_evidence(
        RV_RECEIPT_LIFECYCLE_NOW_MS,
        &decision,
        trigger,
        false,
    );

    strategy.last_recorded_exit_decision = None;
    strategy.last_exit_evidence_outcome.clear();
    replace_rv_with_distinguishable_snapshot(&mut strategy);
    strategy
        .record_exit_decision_once(RV_RECEIPT_LIFECYCLE_NOW_MS + 1, trigger, &decision)
        .expect("post-replacement exit decision evidence should record");
    strategy.record_exit_evaluation_evidence(
        RV_RECEIPT_LIFECYCLE_NOW_MS + 1,
        &decision,
        trigger,
        false,
    );

    let decisions = recorded_exit_decisions(&evidence);
    let evaluations = recorded_exit_evaluations(&evidence);
    assert_eq!(decisions.len(), 2);
    assert_eq!(evaluations.len(), 2);

    let expected_decision_blockers = vec![
        crate::bolt_v3_current_evidence::RealizedVolBlockReason::InvalidConfig,
        crate::bolt_v3_current_evidence::RealizedVolBlockReason::QuorumNotReady,
        crate::bolt_v3_current_evidence::RealizedVolBlockReason::SourceStale,
        crate::bolt_v3_current_evidence::RealizedVolBlockReason::CoverageBelowMinimum,
        crate::bolt_v3_current_evidence::RealizedVolBlockReason::InterSampleGapExceeded,
        crate::bolt_v3_current_evidence::RealizedVolBlockReason::SourceClassMismatch,
        crate::bolt_v3_current_evidence::RealizedVolBlockReason::SampleKindMismatch,
        crate::bolt_v3_current_evidence::RealizedVolBlockReason::CrossSourceDispersion,
        crate::bolt_v3_current_evidence::RealizedVolBlockReason::AnnualizationBasisInvalid,
        crate::bolt_v3_current_evidence::RealizedVolBlockReason::NotWarm,
    ];
    assert_eq!(
        decisions[0].details().rv_snapshot_blockers,
        expected_decision_blockers
    );
    assert_eq!(
        decisions[1].details().rv_snapshot_blockers,
        expected_decision_blockers
    );
    assert_eq!(evaluations[0].rv_blockers, expected_decision_blockers);
    assert_eq!(evaluations[1].rv_blockers, expected_decision_blockers);
    assert!(
        decisions[0].details().rv_snapshot_ready,
        "raw readiness must stay true"
    );
    assert_eq!(
        decisions[0].details().realized_vol,
        None,
        "gate-filtered RV stays absent"
    );
    assert!(!evaluations[0].rv_ready, "usable readiness must stay false");
    assert_eq!(
        decisions[0].details().rv_gate_result,
        RvGateResult::RejectedNotReady
    );
    assert_eq!(
        evaluations[0].rv_gate_result,
        RvGateResult::RejectedNotReady
    );
    assert_eq!(
        decisions[0].details().exit_eval_now_ms,
        RV_RECEIPT_LIFECYCLE_NOW_MS
    );
    assert_eq!(evaluations[0].exit_eval_now_ms, 1_260);
    assert_eq!(
        decisions[0].details().trigger_ts_event_ms,
        RV_RECEIPT_TRIGGER_EVENT_MS
    );
    assert_eq!(evaluations[0].trigger_ts_event_ms, Some(1_100));
    assert_eq!(
        decisions[0].details().rv_snapshot_as_of_ms,
        Some(RV_RECEIPT_SNAPSHOT_AS_OF_MS)
    );
    assert_eq!(
        decisions[0].details().rv_snapshot_receive_watermark_ms,
        Some(RV_RECEIPT_WATERMARK_MS)
    );
    assert_eq!(
        decisions[0].details().trigger_ts_init_ms,
        Some(RV_RECEIPT_TRIGGER_RECEIVE_MS)
    );
    assert_eq!(evaluations[0].rv_as_of_ms, Some(1_350));
    assert_eq!(evaluations[0].rv_snapshot_receive_watermark_ms, Some(1_180));
    assert_eq!(evaluations[0].trigger_ts_init_ms, Some(1_220));
    assert_eq!(decisions[0].details().rv_future_dating_delta_ms, Some(250));
    assert_eq!(evaluations[0].rv_as_of_minus_now_ms, Some(250));

    macro_rules! assert_fields_equal {
        ($before:expr, $after:expr, $($field:ident),+ $(,)?) => {
            $(assert_eq!(
                $before.$field,
                $after.$field,
                "`{}` must remain immutable after snapshot replacement",
                stringify!($field)
            );)+
        };
    }
    let decision_before = decisions[0].details();
    let decision_after = decisions[1].details();
    assert_fields_equal!(
        decision_before,
        decision_after,
        trigger_ts_init_ms,
        rv_surface_id,
        rv_snapshot_as_of_ms,
        rv_snapshot_receive_watermark_ms,
        rv_max_source_age_ms,
        rv_snapshot_ready,
        rv_snapshot_has_ready_realized_vol,
        realized_vol,
        realized_vol_source_venue,
        realized_vol_source_ts_ms,
        rv_snapshot_blockers,
        rv_source_diagnostics,
        rv_gate_result,
        rv_future_dating_delta_ms,
        fair_probability_up,
        fair_probability_down,
        uncertainty_band_probability,
    );
    assert_fields_equal!(
        evaluations[0],
        evaluations[1],
        trigger_ts_init_ms,
        rv_surface_id,
        rv_as_of_ms,
        rv_snapshot_receive_watermark_ms,
        rv_max_source_age_ms,
        rv_ready,
        rv_blockers,
        rv_source_diagnostics,
        rv_gate_result,
        rv_as_of_minus_now_ms,
        fair_probability_up,
        fair_probability_down,
        uncertainty_band_probability,
    );
}

#[test]
fn rv_clock_domain_amendment_valid_surface_without_snapshot_keeps_forced_flat_exit() {
    let (mut strategy, evidence) = exit_evidence_strategy_with_open_position();
    strategy.pricing.clear_latest_realized_vol_snapshot();
    strategy.active.phase = SelectionPhase::Freeze;
    let trigger = ExitEvaluationTriggerContext::new(
        crate::bolt_v3_current_evidence::ExitTriggerSource::SignalQuote,
        1_200,
        Some(1_200),
    );
    let decision = strategy.exit_submission_decision_for_trigger_at(1_200, trigger);
    assert_eq!(decision.evaluation.exit_decision, Some(ExitDecision::Exit));
    assert_eq!(decision.blocked_reason, None);
    strategy
        .record_exit_decision_once(1_200, trigger, &decision)
        .expect("forced-flat decision should record without an RV snapshot");
    strategy.record_exit_evaluation_evidence(1_200, &decision, trigger, false);

    let decisions = recorded_exit_decisions(&evidence);
    let evaluations = recorded_exit_evaluations(&evidence);
    let decision = decisions[0].details();
    let evaluation = &evaluations[0];
    assert_eq!(decision.rv_gate_result, RvGateResult::MissingSnapshot);
    assert_eq!(evaluation.rv_gate_result, RvGateResult::MissingSnapshot);
    assert_eq!(decision.rv_max_source_age_ms, Some(500));
    assert_eq!(evaluation.rv_max_source_age_ms, Some(500));
    assert_eq!(decision.rv_snapshot_has_ready_realized_vol, Some(false));
    assert_eq!(decision.rv_snapshot_receive_watermark_ms, None);
    assert_eq!(evaluation.rv_snapshot_receive_watermark_ms, None);
}

fn rv_clock_domain_amendment_assert_one_field_error(
    strategy_id: &str,
    field: &str,
    action: impl FnOnce(),
) {
    let ((), logs) = with_captured_strategy_logs(strategy_id, action);
    let errors = logs
        .into_iter()
        .filter(|(level, message)| *level == log::Level::Error && message.contains("exit evidence"))
        .collect::<Vec<_>>();
    assert_eq!(
        errors.len(),
        1,
        "conversion/build failure must log exactly one exit-evidence error for `{field}`: {errors:?}"
    );
    assert!(
        errors[0].1.contains(field),
        "field-specific exit-evidence error must name `{field}` exactly: {:?}",
        errors[0]
    );
}

#[test]
fn rv_clock_domain_amendment_exit_evaluation_conversion_failure_skips_record() {
    const FILTER: &str =
        "rv_clock_domain_amendment_exit_evaluation_conversion_failure_skips_record";
    let mode = std::env::var(LOG_CAPTURE_CHILD_ENV).ok();
    if mode
        .as_deref()
        .is_none_or(|mode| !mode.starts_with("conversion-"))
    {
        for mode in [
            "conversion-trigger-event",
            "conversion-trigger-init",
            "conversion-exit-now",
            "conversion-rv-as-of",
            "conversion-watermark",
        ] {
            run_log_capture_test_in_subprocess(FILTER, mode);
        }
        return;
    }
    let mode = mode.expect("conversion child mode should be present");
    let field = match mode.as_str() {
        "conversion-trigger-event" => "trigger_ts_event_ms",
        "conversion-trigger-init" => "trigger_ts_init_ms",
        "conversion-exit-now" => "exit_eval_now_ms",
        "conversion-rv-as-of" => "rv_as_of_ms",
        "conversion-watermark" => "rv_snapshot_receive_watermark_ms",
        _ => unreachable!("parent filters conversion child modes"),
    };

    let (mut strategy, evidence) = exit_evidence_strategy_with_open_position();
    let strategy_id = unique_log_capture_strategy_id(mode.as_str());
    strategy.config.strategy_id = strategy_id.clone();
    let (as_of_ms, watermark_ms) = match mode.as_str() {
        "conversion-rv-as-of" => (u64::MAX, Some(1_200)),
        "conversion-watermark" => (1_200, Some(u64::MAX)),
        _ => (1_200, Some(1_200)),
    };
    rv_clock_domain_amendment_set_snapshot_times(&mut strategy, as_of_ms, watermark_ms);
    let trigger = ExitEvaluationTriggerContext::new(
        crate::bolt_v3_current_evidence::ExitTriggerSource::SignalQuote,
        if mode == "conversion-trigger-event" {
            u64::MAX
        } else {
            1_200
        },
        Some(if mode == "conversion-trigger-init" {
            u64::MAX
        } else {
            1_200
        }),
    );
    let now_ms = if mode == "conversion-exit-now" {
        u64::MAX
    } else {
        1_200
    };
    let decision = strategy.exit_submission_decision_for_trigger_at(1_200, trigger);
    let decision_before = decision.clone();
    let exposure_before = strategy.exposure.clone();

    rv_clock_domain_amendment_assert_one_field_error(&strategy_id, field, || {
        strategy.record_exit_evaluation_evidence(now_ms, &decision, trigger, false);
    });

    assert_eq!(
        decision, decision_before,
        "evidence failure cannot mutate the order result"
    );
    assert_eq!(
        strategy.exposure, exposure_before,
        "evidence failure cannot mutate exposure/submission state"
    );
    assert!(
        recorded_exit_evaluations(&evidence).is_empty(),
        "a failed complete-record build must not leave a partial record or reach the writer"
    );
}

#[test]
fn rv_clock_domain_amendment_exit_evidence_failure_is_non_aborting() {
    const FILTER: &str = "rv_clock_domain_amendment_exit_evidence_failure_is_non_aborting";
    let mode = std::env::var(LOG_CAPTURE_CHILD_ENV).ok();
    if !matches!(
        mode.as_deref(),
        Some("evidence-builder" | "evidence-writer")
    ) {
        run_log_capture_test_in_subprocess(FILTER, "evidence-builder");
        run_log_capture_test_in_subprocess(FILTER, "evidence-writer");
        return;
    }
    let mode = mode.expect("evidence child mode should be present");

    if mode == "evidence-builder" {
        let (mut strategy, evidence) = exit_evidence_strategy_with_open_position();
        let strategy_id = strategy.config.strategy_id.clone();
        rv_clock_domain_amendment_set_snapshot_times(&mut strategy, 1_200, Some(1_200));
        let trigger = ExitEvaluationTriggerContext::new(
            crate::bolt_v3_current_evidence::ExitTriggerSource::SignalQuote,
            u64::MAX,
            Some(1_200),
        );
        let decision = strategy.exit_submission_decision_for_trigger_at(1_200, trigger);
        let exposure_before = strategy.exposure.clone();
        rv_clock_domain_amendment_assert_one_field_error(
            &strategy_id,
            "trigger_ts_event_ms",
            || {
                strategy.record_exit_evaluation_evidence(1_200, &decision, trigger, false);
                strategy.record_exit_evaluation_evidence(1_200, &decision, trigger, false);
            },
        );
        assert_eq!(strategy.exposure, exposure_before);
        assert!(recorded_exit_evaluations(&evidence).is_empty());
        assert_eq!(
            strategy.last_exit_evidence_outcome.len(),
            1,
            "builder failure must preserve mark-before-failure flood protection"
        );
        return;
    }

    let writer = failing_observation_evidence();
    let mut strategy = exit_evidence_strategy_with_open_position_using_writer(writer.clone());
    let strategy_id = strategy.config.strategy_id.clone();
    rv_clock_domain_amendment_set_snapshot_times(&mut strategy, 1_200, Some(1_200));
    let trigger = ExitEvaluationTriggerContext::new(
        crate::bolt_v3_current_evidence::ExitTriggerSource::SignalQuote,
        1_200,
        Some(1_200),
    );
    let decision = strategy.exit_submission_decision_for_trigger_at(1_200, trigger);
    let exposure_before = strategy.exposure.clone();
    rv_clock_domain_amendment_assert_one_field_error(&strategy_id, "write failed", || {
        strategy.record_exit_evaluation_evidence(1_200, &decision, trigger, false);
        strategy.record_exit_evaluation_evidence(1_200, &decision, trigger, false);
    });
    assert_eq!(strategy.exposure, exposure_before);
    assert_eq!(
        writer.attempts_for(
            crate::bolt_v3_current_evidence::generated_contract::KnownPurpose::ExitEvaluation
        ),
        1,
        "writer failure must remain mark-before-failure and non-retrying"
    );
}

#[test]
fn rv_clock_domain_amendment_extreme_exit_deltas_are_lossless() {
    for (
        label,
        snapshot_as_of_ms,
        trigger_event_ms,
        watermark_ms,
        trigger_receive_ms,
        expected_receipt_delta,
    ) in [
        (
            "positive",
            u64::MAX,
            0,
            1_200,
            1_200,
            Some(i128::from(u64::MAX)),
        ),
        ("negative", 0, u64::MAX, 0, 0, Some(-i128::from(u64::MAX))),
    ] {
        let (mut strategy, evidence) = exit_evidence_strategy_with_open_position();
        rv_clock_domain_amendment_set_snapshot_times(
            &mut strategy,
            snapshot_as_of_ms,
            Some(watermark_ms),
        );
        let trigger = ExitEvaluationTriggerContext::new(
            crate::bolt_v3_current_evidence::ExitTriggerSource::SignalQuote,
            trigger_event_ms,
            Some(trigger_receive_ms),
        );
        let decision = strategy.exit_submission_decision_for_trigger_at(1_200, trigger);
        assert_eq!(
            decision
                .evaluation
                .realized_volatility_receipt
                .snapshot_as_of_minus_trigger_event_ms,
            expected_receipt_delta,
            "{label} receipt must retain the raw signed i128 event-time delta"
        );
        let outcome_before = (
            decision.evaluation.exit_decision,
            decision.blocked_reason,
            strategy.exposure.clone(),
        );
        strategy
            .record_exit_decision_once(1_200, trigger, &decision)
            .expect("extreme delta must not abort decision evidence");
        strategy.record_exit_evaluation_evidence(1_200, &decision, trigger, false);

        let expected_delta = i128::from(snapshot_as_of_ms) - i128::from(trigger_event_ms);
        let expected_positive = u64::try_from(expected_delta)
            .ok()
            .filter(|delta| *delta > 0);
        let decisions = recorded_exit_decisions(&evidence);
        assert_eq!(decisions.len(), 1, "{label} decision record should exist");
        assert_eq!(
            decisions[0].details().rv_future_dating_delta_ms,
            expected_positive,
            "{label} decision delta must not wrap or invert sign"
        );
        assert!(
            recorded_exit_evaluations(&evidence).is_empty(),
            "{label} evaluation record must be skipped when its signed wire domain is impossible"
        );
        assert_eq!(
            (
                decision.evaluation.exit_decision,
                decision.blocked_reason,
                strategy.exposure.clone(),
            ),
            outcome_before,
            "{label} evidence conversion cannot change callback/order/exposure outcome"
        );
    }
}

fn rv_clock_domain_amendment_rv_states() -> [(RvGateResult, Option<LocalReceiveMs>); 12] {
    [
        (RvGateResult::Accepted, None),
        (RvGateResult::Accepted, Some(LocalReceiveMs::new(1_200))),
        (RvGateResult::MissingSnapshot, None),
        (
            RvGateResult::MissingSnapshot,
            Some(LocalReceiveMs::new(1_200)),
        ),
        (RvGateResult::MissingEvaluationEventTime, None),
        (
            RvGateResult::MissingEvaluationEventTime,
            Some(LocalReceiveMs::new(1_200)),
        ),
        (RvGateResult::RejectedFutureDated, None),
        (
            RvGateResult::RejectedFutureDated,
            Some(LocalReceiveMs::new(1_200)),
        ),
        (RvGateResult::RejectedStale, None),
        (
            RvGateResult::RejectedStale,
            Some(LocalReceiveMs::new(1_200)),
        ),
        (RvGateResult::RejectedNotReady, None),
        (
            RvGateResult::RejectedNotReady,
            Some(LocalReceiveMs::new(1_200)),
        ),
    ]
}

fn rv_clock_domain_amendment_rv_bit(gate: RvGateResult, watermark_present: bool) -> u16 {
    let gate_index = match gate {
        RvGateResult::Accepted => 0,
        RvGateResult::MissingSnapshot => 1,
        RvGateResult::MissingEvaluationEventTime => 2,
        RvGateResult::RejectedFutureDated => 3,
        RvGateResult::RejectedStale => 4,
        RvGateResult::RejectedNotReady => 5,
    };
    let watermark_index = if watermark_present { 1 } else { 0 };
    1_u16 << (gate_index * 2 + watermark_index)
}

fn blocked_rv_metadata(
    record: &crate::bolt_v3_current_evidence::BlockedStrategyInputObservationFact,
) -> (RvGateResult, Option<u64>) {
    match &record.details.realized_volatility {
        StrategyInputRvState::Absent {
            gate_result,
            receive_watermark_ms,
        } => (*gate_result, *receive_watermark_ms),
        StrategyInputRvState::Present {
            gate_result,
            receive_watermark_ms,
            ..
        } => (*gate_result, *receive_watermark_ms),
    }
}

fn rv_clock_domain_amendment_entry_mask(events: &[CurrentFact]) -> u16 {
    events.iter().fold(0_u16, |mask, event| match event {
        CurrentFact::EntrySkipObservation(skip) => {
            skip.realized_vol_gate_result.map_or(mask, |gate| {
                mask | rv_clock_domain_amendment_rv_bit(
                    gate,
                    skip.realized_vol_receive_watermark_ms.is_some(),
                )
            })
        }
        _ => mask,
    })
}

fn rv_clock_domain_amendment_blocked_mask(events: &[CurrentFact]) -> u16 {
    events.iter().fold(0_u16, |mask, event| match event {
        CurrentFact::BlockedStrategyInputObservation(snapshot) => {
            let (gate, watermark_present) = match &snapshot.details.realized_volatility {
                StrategyInputRvState::Absent {
                    gate_result,
                    receive_watermark_ms,
                } => (*gate_result, receive_watermark_ms.is_some()),
                StrategyInputRvState::Present {
                    gate_result,
                    receive_watermark_ms,
                    ..
                } => (*gate_result, receive_watermark_ms.is_some()),
            };
            mask | rv_clock_domain_amendment_rv_bit(gate, watermark_present)
        }
        _ => mask,
    })
}

fn rv_clock_domain_amendment_dedupe_strategy(
    evidence: Arc<crate::bolt_v3_current_evidence::DecisionEvidenceRecorder>,
) -> BinaryOracleEdgeTaker {
    let submit_admission = Arc::new(
        crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new(evidence.clone()),
    );
    let mut strategy = ready_to_trade_strategy_with_decision_evidence_and_submit_admission(
        evidence,
        submit_admission,
    );
    register_test_strategy_with_active_instruments(&mut strategy);
    strategy
}

fn rv_clock_domain_amendment_blocked_decision() -> EntrySubmissionDecision {
    let mut decision = minimal_entry_submission_decision();
    decision.evaluation.pricing_blocked_by = vec![EntryPricingBlockReason::RealizedVolNotReady];
    decision
        .evaluation
        .realized_volatility_receipt
        .evidence
        .surface_id = TEST_SURFACE_ID.to_string();
    decision
        .evaluation
        .realized_volatility_receipt
        .evidence
        .blockers = vec![crate::bolt_v3_current_evidence::RealizedVolBlockReason::QuorumNotReady];
    decision
}

fn rv_clock_domain_amendment_apply_state(
    decision: &mut EntrySubmissionDecision,
    gate: RvGateResult,
    watermark: Option<LocalReceiveMs>,
) {
    decision.evaluation.realized_volatility_receipt.gate_result = gate;
    decision
        .evaluation
        .realized_volatility_receipt
        .receive_watermark_ms = watermark;
}

#[test]
fn rv_clock_domain_amendment_entry_skip_records_each_registered_reason_once() {
    // Re-aimed at this producer's actual domain. Entry skip used to key on the
    // twelve RV gate/watermark bits, which is not its axis under the frozen
    // registry -- that domain belongs to the blocked-snapshot producer, whose
    // own twelve-bit test still stands beside this one. Entry skip's registered
    // domain is the twenty skip reasons, so that is what is exercised here, and
    // exhaustively: a reason added to the enum without a registry state fails
    // the mapping's exhaustive match at compile time, and a reason that stops
    // recording fails the count below.
    const REGISTERED_REASONS: [EntrySkipReason; 20] = [
        EntrySkipReason::StrategyCoreNotRegistered,
        EntrySkipReason::EntryGateBlocked,
        EntrySkipReason::EntryPricingBlocked,
        EntrySkipReason::NoSideSelected,
        EntrySkipReason::SizedNotionalNotPositive,
        EntrySkipReason::InstrumentIdMissing,
        EntrySkipReason::InstrumentMissingFromCache,
        EntrySkipReason::EntryPriceMissing,
        EntrySkipReason::QuantityRoundingFailed,
        EntrySkipReason::LimitNotionalExceedsSizedNotional,
        EntrySkipReason::EntryQuoteNotionalBelowVenueMinimum,
        EntrySkipReason::EntryQuoteNotionalMinimumUnmodeled,
        EntrySkipReason::QuantityNotPositive,
        EntrySkipReason::PositionContractInvalid,
        EntrySkipReason::EntryPositionContractUnsupported,
        EntrySkipReason::HistoricalEntryFeeUnavailable,
        EntrySkipReason::OnePositionInvariantViolation,
        EntrySkipReason::EntryMalformedRejected,
        EntrySkipReason::EntryBalanceRejected,
        EntrySkipReason::EntryUnfillableRejectedUnchangedBook,
    ];

    let evidence = recording_decision_evidence();
    let mut strategy = rv_clock_domain_amendment_dedupe_strategy(evidence.clone());
    let mut decision = minimal_entry_submission_decision();

    // Every reason once, then every reason a second time with the RV state
    // churning underneath. The second pass must add nothing: the reasons are
    // already claimed for this episode, and RV state is not this producer's axis.
    let rv_states = rv_clock_domain_amendment_rv_states();
    let mut now_ms = 1_200_u64;
    for pass in 0..2 {
        for (index, reason) in REGISTERED_REASONS.into_iter().enumerate() {
            let (gate, watermark) = rv_states[(index + pass) % rv_states.len()];
            rv_clock_domain_amendment_apply_state(&mut decision, gate, watermark);
            strategy
                .record_entry_skip_once(now_ms, &decision, reason)
                .expect("a suppressed skip is still a successful call");
            now_ms += 1;
        }
    }

    let recorded = evidence
        .recorded_facts()
        .expect("recorded current evidence must decode")
        .into_iter()
        .filter_map(|event| match event {
            CurrentFact::EntrySkipObservation(skip) => Some(skip.reason_category),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        recorded.as_slice(),
        &REGISTERED_REASONS[..],
        "each registered skip reason must record exactly once per market episode, in the \
         order first observed, and a second pass under churning RV state must add nothing"
    );
}

#[test]
fn rv_clock_domain_amendment_blocked_snapshot_current_key_tracks_twelve_rv_bits() {
    let evidence = recording_decision_evidence();
    let mut strategy = rv_clock_domain_amendment_dedupe_strategy(evidence.clone());
    let mut decision = rv_clock_domain_amendment_blocked_decision();

    for (index, (gate, watermark)) in rv_clock_domain_amendment_rv_states()
        .into_iter()
        .enumerate()
    {
        rv_clock_domain_amendment_apply_state(&mut decision, gate, watermark);
        strategy
            .record_blocked_entry_strategy_input_snapshot_once(1_200 + index as u64, &decision)
            .expect("each unseen blocked-snapshot RV category/presence bit should record");
    }

    let events = evidence
        .recorded_facts()
        .expect("recorded current evidence must decode");
    let mask = rv_clock_domain_amendment_blocked_mask(&events);
    assert_eq!(
        mask.count_ones(),
        12,
        "all twelve blocked-snapshot RV bits must emit once"
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, CurrentFact::BlockedStrategyInputObservation(_)))
            .count(),
        12
    );
}

#[test]
fn rv_clock_domain_amendment_input_churn_does_not_reclaim_a_seen_state() {
    // Inverted deliberately. This test used to assert that every key transition
    // started a fresh mask, so A-to-B-to-A re-emitted every previously seen RV
    // state. That is the behaviour #1354's frozen amendment forbids in as many
    // words -- "input churn must never reclaim a seen canonical state or reset
    // suppression" -- so the sequences below are unchanged and only the expected
    // cardinality moved. What made the old numbers possible was keying the
    // episode on values that churn; the episode is now the market itself.
    let rv_states = [
        (RvGateResult::Accepted, Some(LocalReceiveMs::new(1_200))),
        (
            RvGateResult::RejectedStale,
            Some(LocalReceiveMs::new(1_200)),
        ),
    ];

    // Entry skip: the novelty axis is the skip reason, so RV churn underneath it
    // is invisible and a returning reason is already claimed.
    let entry_evidence = recording_decision_evidence();
    let mut entry_strategy = rv_clock_domain_amendment_dedupe_strategy(entry_evidence.clone());
    let mut entry = minimal_entry_submission_decision();
    let mut now_ms = 1_200_u64;
    for category in [
        EntrySkipReason::EntryPricingBlocked,
        EntrySkipReason::NoSideSelected,
        EntrySkipReason::EntryPricingBlocked,
    ] {
        for (gate, watermark) in rv_states {
            rv_clock_domain_amendment_apply_state(&mut entry, gate, watermark);
            entry_strategy
                .record_entry_skip_once(now_ms, &entry, category)
                .expect("a suppressed skip is still a successful call");
            now_ms += 1;
        }
    }
    let entry_reasons = entry_evidence
        .recorded_facts()
        .expect("recorded current evidence must decode")
        .into_iter()
        .filter_map(|event| match event {
            CurrentFact::EntrySkipObservation(skip) => Some(skip.reason_category),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        entry_reasons.as_slice(),
        &[
            EntrySkipReason::EntryPricingBlocked,
            EntrySkipReason::NoSideSelected
        ][..],
        "six evaluations over two distinct skip reasons must record twice: the returning \
         reason is already claimed, and the RV churn beneath it is not this producer's axis"
    );

    // Blocked snapshot: the novelty axis is the RV state, and the market id the
    // old key carried is no longer what identifies the episode -- so churning it
    // must change nothing.
    let blocked_evidence = recording_decision_evidence();
    let mut blocked_strategy = rv_clock_domain_amendment_dedupe_strategy(blocked_evidence.clone());
    let mut blocked = rv_clock_domain_amendment_blocked_decision();
    let original_market_id = blocked_strategy.active.market_id.clone();
    let mut now_ms = 1_300_u64;
    for market_id in [
        original_market_id.clone(),
        Some("<KEY_B>".to_string()),
        original_market_id,
    ] {
        blocked_strategy.active.market_id = market_id;
        for (gate, watermark) in rv_states {
            rv_clock_domain_amendment_apply_state(&mut blocked, gate, watermark);
            blocked_strategy
                .record_blocked_entry_strategy_input_snapshot_once(now_ms, &blocked)
                .expect("a suppressed snapshot is still a successful call");
            now_ms += 1;
        }
    }
    assert_eq!(
        rv_clock_domain_amendment_blocked_rv_states(&blocked_evidence).len(),
        2,
        "two distinct RV states over six evaluations must record twice however often the \
         market id churns underneath them"
    );

    // The other direction, which the old test had no way to express: a genuinely
    // different market is a different episode and starts with nothing claimed.
    // Without this, "churn changes nothing" would also be satisfied by a guard
    // that never records again at all.
    let first_identity = blocked_strategy
        .active
        .evidence_identity
        .clone()
        .expect("the dedupe fixture must bind a market identity");
    let other_identity =
        SelectedMarketEvidenceIdentity::try_new(
            first_identity.market().gamma_market_id().to_string(),
            format!("{}-second-market", first_identity.market().condition_id()),
            first_identity.market().question_id().to_string(),
            first_identity.negative_risk(),
            first_identity.market().outcomes().clone().map(|outcome| {
                SelectedMarketEvidenceOutcome {
                    index: outcome.index,
                    normalized_outcome: outcome.normalized_outcome,
                    clob_token_id: outcome.clob_token_id,
                }
            }),
        )
        .expect("the second market identity must remain valid");
    blocked_strategy.active.evidence_identity = Some(other_identity);
    for (gate, watermark) in rv_states {
        rv_clock_domain_amendment_apply_state(&mut blocked, gate, watermark);
        blocked_strategy
            .record_blocked_entry_strategy_input_snapshot_once(now_ms, &blocked)
            .expect("a new market episode records");
        now_ms += 1;
    }
    assert_eq!(
        rv_clock_domain_amendment_blocked_rv_states(&blocked_evidence).len(),
        4,
        "a different market is a different episode, so its own two RV states record again"
    );

    // And back to the first market. This is the assertion the amendment's
    // sentence actually requires -- "input churn must never reclaim a seen
    // canonical state" is about returning to an episode, not merely leaving it.
    // Without this the whole suite tolerates a single-slot guard that evicts the
    // previous episode, because nothing ever revisits one. Mutation-checked: a
    // guard that keeps only the current episode fails here.
    blocked_strategy.active.evidence_identity = Some(first_identity);
    for (gate, watermark) in rv_states {
        rv_clock_domain_amendment_apply_state(&mut blocked, gate, watermark);
        blocked_strategy
            .record_blocked_entry_strategy_input_snapshot_once(now_ms, &blocked)
            .expect("a returning episode is still a successful call");
        now_ms += 1;
    }
    assert_eq!(
        rv_clock_domain_amendment_blocked_rv_states(&blocked_evidence).len(),
        4,
        "returning to the first market must add nothing: its states were claimed and a later \
         episode must not have reclaimed them"
    );
}

/// The RV states recorded on blocked-snapshot observations so far.
fn rv_clock_domain_amendment_blocked_rv_states(
    evidence: &Arc<crate::bolt_v3_current_evidence::DecisionEvidenceRecorder>,
) -> Vec<(RvGateResult, bool)> {
    evidence
        .recorded_facts()
        .expect("recorded current evidence must decode")
        .into_iter()
        .filter_map(|event| match event {
            CurrentFact::BlockedStrategyInputObservation(snapshot) => {
                let (gate, watermark_present) = match snapshot.details.realized_volatility {
                    StrategyInputRvState::Absent {
                        gate_result,
                        receive_watermark_ms,
                    } => (gate_result, receive_watermark_ms.is_some()),
                    StrategyInputRvState::Present {
                        gate_result,
                        receive_watermark_ms,
                        ..
                    } => (gate_result, receive_watermark_ms.is_some()),
                };
                Some((gate, watermark_present))
            }
            _ => None,
        })
        .collect()
}

#[test]
fn selecting_a_market_binds_the_episode_identity_every_producer_keys_on() {
    // Pins the seam no other test covers. Every episode test binds
    // `active.evidence_identity` by hand, so deleting the copy in
    // `ActiveMarketState::from_market` would leave all of them green while
    // production silently lost every episode record -- each producer would find
    // no episode, treat its observation as unattributable, and record nothing.
    // That failure is invisible by construction, which is exactly why it needs a
    // test that goes through selection rather than around it.
    let evidence = recording_decision_evidence();
    let mut strategy = test_strategy_with_fee_provider_and_decision_evidence(
        RecordingFeeProvider::cold(),
        evidence,
    );

    strategy.active.evidence_identity = None;
    assert!(
        strategy.evidence_episode_id().is_err(),
        "with nothing selected there is no episode to attribute evidence to"
    );

    let market = candidate_market("wiring-market", 1_200);
    let expected = market.evidence_identity.clone();
    strategy.apply_selection_snapshot(selection_snapshot(
        1_200,
        SelectionState::Active {
            market: Box::new(market),
        },
    ));

    assert_eq!(
        strategy.active.evidence_identity.as_ref(),
        Some(&expected),
        "selecting a market must carry its evidence identity onto the active state"
    );
    assert!(
        strategy.evidence_episode_id().is_ok(),
        "a selected market must yield the episode its producers key on"
    );
}

#[test]
fn rv_clock_domain_amendment_current_key_rv_mask_suppresses_repeats() {
    let entry_evidence = recording_decision_evidence();
    let blocked_evidence = recording_decision_evidence();
    let mut entry_strategy = rv_clock_domain_amendment_dedupe_strategy(entry_evidence.clone());
    let mut blocked_strategy = rv_clock_domain_amendment_dedupe_strategy(blocked_evidence.clone());
    let mut entry = minimal_entry_submission_decision();
    let mut blocked = rv_clock_domain_amendment_blocked_decision();

    for (index, (gate, watermark)) in rv_clock_domain_amendment_rv_states()
        .into_iter()
        .enumerate()
    {
        rv_clock_domain_amendment_apply_state(&mut entry, gate, watermark);
        rv_clock_domain_amendment_apply_state(&mut blocked, gate, watermark);
        entry_strategy
            .record_entry_skip_once(
                1_200 + index as u64,
                &entry,
                EntrySkipReason::EntryPricingBlocked,
            )
            .unwrap();
        blocked_strategy
            .record_blocked_entry_strategy_input_snapshot_once(1_200 + index as u64, &blocked)
            .unwrap();
    }

    for index in 0..128_u64 {
        let (gate, watermark) = if index % 2 == 0 {
            (RvGateResult::Accepted, Some(LocalReceiveMs::new(1_201)))
        } else {
            (RvGateResult::RejectedNotReady, None)
        };
        rv_clock_domain_amendment_apply_state(&mut entry, gate, watermark);
        rv_clock_domain_amendment_apply_state(&mut blocked, gate, watermark);
        entry_strategy
            .record_entry_skip_once(1_300 + index, &entry, EntrySkipReason::EntryPricingBlocked)
            .unwrap();
        blocked_strategy
            .record_blocked_entry_strategy_input_snapshot_once(1_300 + index, &blocked)
            .unwrap();
    }

    let entry_events = entry_evidence
        .recorded_facts()
        .expect("recorded current evidence must decode");
    let blocked_events = blocked_evidence
        .recorded_facts()
        .expect("recorded current evidence must decode");
    // One RV state reaches the entry stream, not twelve: the single skip reason
    // used throughout claims its state on the first observation, and RV is not
    // this producer's axis. The blocked producer's axis *is* RV, so all twelve
    // of its states still appear -- the two together are why the split exists.
    assert_eq!(
        rv_clock_domain_amendment_entry_mask(&entry_events).count_ones(),
        1
    );
    assert_eq!(
        rv_clock_domain_amendment_blocked_mask(&blocked_events).count_ones(),
        12
    );
    assert_eq!(
        entry_events
            .iter()
            .filter(|event| matches!(event, CurrentFact::EntrySkipObservation(_)))
            .count(),
        1,
        "100+ repeats and A-B-A oscillations over one skip reason must remain bounded to a \
         single entry record -- the cardinality is fixed by the registry domain the producer \
         is registered against, not by how long the oscillation runs"
    );
    assert_eq!(
        blocked_events
            .iter()
            .filter(|event| matches!(event, CurrentFact::BlockedStrategyInputObservation(_)))
            .count(),
        12,
        "100+ repeats and A-B-A oscillations must remain bounded to twelve blocked records"
    );
    assert!(entry_events.iter().all(|event| {
        match event {
            CurrentFact::EntrySkipObservation(skip) => skip
                .realized_vol_receive_watermark_ms
                .is_none_or(|watermark| watermark == 1_200),
            _ => true,
        }
    }));
    assert!(blocked_events.iter().all(|event| {
        match event {
            CurrentFact::BlockedStrategyInputObservation(snapshot) => blocked_rv_metadata(snapshot)
                .1
                .is_none_or(|watermark| watermark == 1_200),
            _ => true,
        }
    }));
}

#[derive(Debug, Clone, Copy)]
enum RvClockDomainBlockedKeyField {
    ConfiguredTargetAndMarketSelectionRulesetIds,
    MarketSelectionOutcome,
    MarketId,
    UpInstrumentId,
    DownInstrumentId,
    PriceToBeatSource,
    GateBlockedBy,
    PricingBlockedBy,
    SelectedSide,
    FastVenueName,
    FastVenueAvailable,
    ReferenceCurrentPriceSourceId,
    ReferenceCurrentPriceAvailable,
    ReferenceCurrentPriceFailedOver,
    FastVenueIncoherent,
    RealizedVolatilitySurfaceId,
    RealizedVolatilityBlockers,
    SourceId,
    SourceEnabled,
    SourceCountsTowardQuorum,
    SourceStatus,
    SourceBlockReason,
    SourceLastRejectedReason,
    UnknownSourceIds,
}

impl RvClockDomainBlockedKeyField {
    const ALL: [Self; 24] = [
        Self::ConfiguredTargetAndMarketSelectionRulesetIds,
        Self::MarketSelectionOutcome,
        Self::MarketId,
        Self::UpInstrumentId,
        Self::DownInstrumentId,
        Self::PriceToBeatSource,
        Self::GateBlockedBy,
        Self::PricingBlockedBy,
        Self::SelectedSide,
        Self::FastVenueName,
        Self::FastVenueAvailable,
        Self::ReferenceCurrentPriceSourceId,
        Self::ReferenceCurrentPriceAvailable,
        Self::ReferenceCurrentPriceFailedOver,
        Self::FastVenueIncoherent,
        Self::RealizedVolatilitySurfaceId,
        Self::RealizedVolatilityBlockers,
        Self::SourceId,
        Self::SourceEnabled,
        Self::SourceCountsTowardQuorum,
        Self::SourceStatus,
        Self::SourceBlockReason,
        Self::SourceLastRejectedReason,
        Self::UnknownSourceIds,
    ];

    fn set_changed(
        self,
        strategy: &mut BinaryOracleEdgeTaker,
        decision: &mut EntrySubmissionDecision,
        changed: bool,
    ) {
        match self {
            // The runtime snapshot derives both serialized fields from this one
            // configured target, so their writer-path transition is necessarily shared.
            Self::ConfiguredTargetAndMarketSelectionRulesetIds => {
                strategy.config.configured_target_id =
                    if changed { "<TARGET_B>" } else { "<TARGET_A>" }.to_string();
            }
            Self::MarketSelectionOutcome => {
                strategy.active.market_selection_outcome = if changed {
                    MarketSelectionOutcome::Next
                } else {
                    MarketSelectionOutcome::Current
                };
            }
            Self::MarketId => {
                strategy.active.market_id =
                    Some(if changed { "<MARKET_B>" } else { "<MARKET_A>" }.to_string());
            }
            Self::UpInstrumentId => {
                strategy.active.books.up.instrument_id = Some(
                    nautilus_model::identifiers::InstrumentId::from(if changed {
                        "condition-B-B-UP.POLYMARKET"
                    } else {
                        "condition-A-A-UP.POLYMARKET"
                    }),
                );
            }
            Self::DownInstrumentId => {
                strategy.active.books.down.instrument_id = Some(
                    nautilus_model::identifiers::InstrumentId::from(if changed {
                        "condition-B-B-DOWN.POLYMARKET"
                    } else {
                        "condition-A-A-DOWN.POLYMARKET"
                    }),
                );
            }
            Self::PriceToBeatSource => {
                strategy.config.price_to_beat_source = if changed {
                    "<PRICE_TO_BEAT_B>"
                } else {
                    "<PRICE_TO_BEAT_A>"
                }
                .to_string();
            }
            Self::GateBlockedBy => {
                decision.evaluation.gate.blocked_by = if changed {
                    vec![EntryBlockReason::PhaseNotActive]
                } else {
                    Vec::new()
                };
            }
            Self::PricingBlockedBy => {
                decision.evaluation.pricing_blocked_by =
                    vec![EntryPricingBlockReason::RealizedVolNotReady];
                if changed {
                    decision
                        .evaluation
                        .pricing_blocked_by
                        .push(EntryPricingBlockReason::SpotPriceMissing);
                }
            }
            Self::SelectedSide => {
                decision.evaluation.selected_side = changed.then_some(OutcomeSide::Up);
            }
            Self::FastVenueName => {
                strategy.latest_signal_quote = None;
                strategy.pricing.set_selected_pricing_spot(Some(fast_spot(
                    if changed { "<FAST_B>" } else { "<FAST_A>" },
                    3_100.5,
                    1_200,
                )));
            }
            Self::FastVenueAvailable => {
                let spot = fast_spot("<FAST_AVAILABILITY>", 3_100.5, 1_200);
                strategy.latest_signal_quote = Some(spot.clone());
                strategy
                    .pricing
                    .set_selected_pricing_spot((!changed).then_some(spot));
            }
            Self::ReferenceCurrentPriceSourceId => {
                strategy.active.reference_current_price_source_id = Some(
                    if changed {
                        "<REFERENCE_B>"
                    } else {
                        "<REFERENCE_A>"
                    }
                    .to_string(),
                );
            }
            Self::ReferenceCurrentPriceAvailable => {
                strategy
                    .pricing
                    .set_last_reference_fair_value((!changed).then_some(3_100.0));
            }
            Self::ReferenceCurrentPriceFailedOver => {
                strategy.active.reference_current_price_failed_over = Some(changed);
            }
            Self::FastVenueIncoherent => {
                strategy.pricing.fast_venue_incoherent = changed;
            }
            Self::RealizedVolatilitySurfaceId => {
                decision
                    .evaluation
                    .realized_volatility_receipt
                    .evidence
                    .surface_id = if changed {
                    "<SURFACE_B>"
                } else {
                    "<SURFACE_A>"
                }
                .to_string();
            }
            Self::RealizedVolatilityBlockers => {
                let blockers = &mut decision
                    .evaluation
                    .realized_volatility_receipt
                    .evidence
                    .blockers;
                *blockers =
                    vec![crate::bolt_v3_current_evidence::RealizedVolBlockReason::QuorumNotReady];
                if changed {
                    blockers
                        .push(crate::bolt_v3_current_evidence::RealizedVolBlockReason::SourceStale);
                }
            }
            Self::SourceId => {
                decision
                    .evaluation
                    .realized_volatility_receipt
                    .evidence
                    .source_diagnostics[0]
                    .source_id = if changed { "<SOURCE_B>" } else { "<SOURCE_A>" }.to_string();
            }
            Self::SourceEnabled => {
                decision
                    .evaluation
                    .realized_volatility_receipt
                    .evidence
                    .source_diagnostics[0]
                    .enabled = !changed;
            }
            Self::SourceCountsTowardQuorum => {
                decision
                    .evaluation
                    .realized_volatility_receipt
                    .evidence
                    .source_diagnostics[0]
                    .counts_toward_quorum = !changed;
            }
            Self::SourceStatus => {
                decision
                    .evaluation
                    .realized_volatility_receipt
                    .evidence
                    .source_diagnostics[0]
                    .status = if changed {
                    crate::bolt_v3_current_evidence::RealizedVolSourceStatus::Ready
                } else {
                    crate::bolt_v3_current_evidence::RealizedVolSourceStatus::Blocked
                };
            }
            Self::SourceBlockReason => {
                decision
                    .evaluation
                    .realized_volatility_receipt
                    .evidence
                    .source_diagnostics[0]
                    .block_reason = Some(if changed {
                    crate::bolt_v3_current_evidence::RealizedVolBlockReason::SourceStale
                } else {
                    crate::bolt_v3_current_evidence::RealizedVolBlockReason::QuorumNotReady
                });
            }
            Self::SourceLastRejectedReason => {
                decision
                    .evaluation
                    .realized_volatility_receipt
                    .evidence
                    .source_diagnostics[0]
                    .last_rejected_reason = changed.then_some(
                    crate::bolt_v3_current_evidence::RealizedVolSourceRejectReason::EventTimeRegression,
                );
            }
            Self::UnknownSourceIds => {
                let rejections = &mut decision
                    .evaluation
                    .realized_volatility_receipt
                    .evidence
                    .unknown_source_rejections;
                rejections.clear();
                rejections.insert("<UNKNOWN_A>".to_string(), 1);
                if changed {
                    rejections.insert("<UNKNOWN_B>".to_string(), 1);
                }
            }
        }
    }
}

fn rv_clock_domain_amendment_assert_blocked_key_field_resets_rv_mask(
    field: RvClockDomainBlockedKeyField,
) {
    let evidence = recording_decision_evidence();
    let mut strategy = rv_clock_domain_amendment_dedupe_strategy(evidence.clone());
    rv_clock_domain_amendment_install_raw_ready_blocked_snapshot(&mut strategy, 1_200, Some(1_200));
    let mut decision = rv_clock_domain_amendment_blocked_decision();
    decision.evaluation.realized_volatility_receipt.evidence =
        strategy.realized_volatility_evidence_fields();
    rv_clock_domain_amendment_apply_state(
        &mut decision,
        RvGateResult::RejectedNotReady,
        Some(LocalReceiveMs::new(1_200)),
    );

    field.set_changed(&mut strategy, &mut decision, false);
    strategy
        .record_blocked_entry_strategy_input_snapshot_once(1_200, &decision)
        .unwrap();
    field.set_changed(&mut strategy, &mut decision, true);
    strategy
        .record_blocked_entry_strategy_input_snapshot_once(1_201, &decision)
        .unwrap();
    field.set_changed(&mut strategy, &mut decision, false);
    strategy
        .record_blocked_entry_strategy_input_snapshot_once(1_202, &decision)
        .unwrap();

    // Most of these fields are diagnostics and cannot reopen an episode. One is
    // not: the configured target id is part of the episode identity the frozen
    // contract defines ("stable logical strategy/target/venue identity"), so
    // changing it genuinely starts a new episode and the same RV state records
    // once more there. Returning to the original target is still suppressed,
    // because that episode already claimed the state -- which is the whole
    // difference between identity and churn, and is why this is a property of
    // the field rather than a tolerance on the count.
    let opens_a_new_episode = matches!(
        field,
        RvClockDomainBlockedKeyField::ConfiguredTargetAndMarketSelectionRulesetIds
    );
    let expected_records = usize::from(opens_a_new_episode) + 1;
    assert_eq!(
        evidence.attempts_for(crate::bolt_v3_current_evidence::generated_contract::KnownPurpose::BlockedStrategyInputObservation),
        expected_records,
        "{field:?}: a suppressed observation is refused before its payload is built, so the \
         writer must see exactly the observations that opened or re-identified an episode"
    );
    let records = evidence
        .recorded_facts()
        .expect("recorded current evidence must decode")
        .into_iter()
        .filter_map(|event| match event {
            CurrentFact::BlockedStrategyInputObservation(snapshot) => Some(snapshot),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        records.len(),
        expected_records,
        "{field:?}: A-to-B-to-A must not reclaim a seen state -- a diagnostic records once, \
         and an identity field records once per distinct episode, never three times"
    );
    assert!(records.iter().all(|record| {
        blocked_rv_metadata(record) == (RvGateResult::RejectedNotReady, Some(1_200))
    }));
}

fn rv_clock_domain_amendment_assert_aliased_blocked_key_fields_are_independent() {
    // Superseded, and deliberately re-aimed rather than deleted. The blocked
    // producer no longer keys on snapshot fields, so "these two serialized
    // fields are independently part of the key" is no longer a property that
    // exists. What replaced it is worth pinning here instead: that this
    // strategy's episode identity is the one the frozen contract defines, built
    // from the active market's own identity and nothing else.
    //
    // The registry's own tests prove which components separate two identities.
    // This one proves Bolt wires the right components in -- the half no test in
    // that file can see.
    let evidence = recording_decision_evidence();
    let strategy = rv_clock_domain_amendment_dedupe_strategy(evidence);
    let identity = strategy
        .active
        .evidence_identity
        .as_ref()
        .expect("the dedupe fixture must bind a market identity");
    let episode = strategy
        .evidence_episode_id()
        .expect("a bound market must yield an episode identity");
    let expected = EvidenceEpisodeId::try_from(EvidenceEpisodeParts {
        strategy_id: strategy.config.strategy_id.clone(),
        target_id: strategy.config.configured_target_id.to_string(),
        venue_id: strategy.context.execution_venue().to_string(),
        market: identity.market().clone(),
    })
    .expect("the fixture's market identity must be constructible");
    assert_eq!(
        episode, expected,
        "the episode identity must be the market's own identity under the frozen contract"
    );
}

#[test]
fn rv_clock_domain_amendment_existing_key_changes_reset_rv_mask() {
    let entry_evidence = recording_decision_evidence();
    let mut entry_strategy = rv_clock_domain_amendment_dedupe_strategy(entry_evidence.clone());
    let mut entry = minimal_entry_submission_decision();
    rv_clock_domain_amendment_apply_state(
        &mut entry,
        RvGateResult::Accepted,
        Some(LocalReceiveMs::new(1_200)),
    );
    let baseline_category = EntrySkipReason::EntryPricingBlocked;
    entry_strategy
        .record_entry_skip_once(1_200, &entry, baseline_category)
        .unwrap();

    let mut next_ms = 1_201_u64;
    let mut record_entry = |strategy: &mut BinaryOracleEdgeTaker,
                            decision: &EntrySubmissionDecision,
                            category: EntrySkipReason| {
        let now_ms = next_ms;
        next_ms += 1;
        strategy
            .record_entry_skip_once(now_ms, decision, category)
            .unwrap();
    };

    record_entry(&mut entry_strategy, &entry, EntrySkipReason::NoSideSelected);
    record_entry(&mut entry_strategy, &entry, baseline_category);

    entry
        .evaluation
        .gate
        .blocked_by
        .push(EntryBlockReason::PhaseNotActive);
    record_entry(&mut entry_strategy, &entry, baseline_category);
    entry.evaluation.gate.blocked_by.clear();
    record_entry(&mut entry_strategy, &entry, baseline_category);

    entry
        .evaluation
        .pricing_blocked_by
        .push(EntryPricingBlockReason::SpotPriceMissing);
    record_entry(&mut entry_strategy, &entry, baseline_category);
    entry.evaluation.pricing_blocked_by.clear();
    record_entry(&mut entry_strategy, &entry, baseline_category);

    let original_market_id = entry_strategy.active.market_id.clone();
    entry_strategy.active.market_id = Some("<ADJACENT_MARKET>".to_string());
    record_entry(&mut entry_strategy, &entry, baseline_category);
    entry_strategy.active.market_id = original_market_id;
    record_entry(&mut entry_strategy, &entry, baseline_category);

    let original_interval_open = entry_strategy.active.interval_open;
    entry_strategy.active.interval_open = Some(3_101.0);
    record_entry(&mut entry_strategy, &entry, baseline_category);
    entry_strategy.active.interval_open = original_interval_open;
    record_entry(&mut entry_strategy, &entry, baseline_category);

    let original_spot = entry_strategy.pricing.selected_pricing_spot().cloned();
    entry_strategy.pricing.set_selected_pricing_spot(None);
    record_entry(&mut entry_strategy, &entry, baseline_category);
    entry_strategy
        .pricing
        .set_selected_pricing_spot(original_spot);
    record_entry(&mut entry_strategy, &entry, baseline_category);

    let original_reference = entry_strategy.pricing.last_reference_current_price();
    let changed_reference = if original_reference.is_some() {
        None
    } else {
        Some(3_101.0)
    };
    entry_strategy
        .pricing
        .set_last_reference_fair_value(changed_reference);
    record_entry(&mut entry_strategy, &entry, baseline_category);
    entry_strategy
        .pricing
        .set_last_reference_fair_value(original_reference);
    record_entry(&mut entry_strategy, &entry, baseline_category);

    entry_strategy.active.fast_venue_incoherent = !entry_strategy.active.fast_venue_incoherent;
    record_entry(&mut entry_strategy, &entry, baseline_category);
    entry_strategy.active.fast_venue_incoherent = !entry_strategy.active.fast_venue_incoherent;
    record_entry(&mut entry_strategy, &entry, baseline_category);

    assert_eq!(
        entry_evidence
            .recorded_facts()
            .expect("recorded current evidence must decode")
            .iter()
            .filter(|event| matches!(event, CurrentFact::EntrySkipObservation(_)))
            .count(),
        2,
        "eight diagnostic fields churned through change-and-restore must record nothing extra. \
         Only the two distinct skip reasons record; every field exercised below -- blockers, \
         market id, interval open, pricing spot, reference price, fast-venue coherence -- is a \
         value that changes while the market does not, which is exactly what the frozen \
         contract excludes from episode identity"
    );

    for field in RvClockDomainBlockedKeyField::ALL {
        rv_clock_domain_amendment_assert_blocked_key_field_resets_rv_mask(field);
    }
    rv_clock_domain_amendment_assert_aliased_blocked_key_fields_are_independent();
}

#[test]
fn rv_clock_domain_amendment_existing_reset_sites_clear_rv_state() {
    let entry_evidence = recording_decision_evidence();
    let mut entry_strategy = rv_clock_domain_amendment_dedupe_strategy(entry_evidence.clone());
    let mut skip = minimal_entry_submission_decision();
    rv_clock_domain_amendment_apply_state(
        &mut skip,
        RvGateResult::Accepted,
        Some(LocalReceiveMs::new(1_200)),
    );
    entry_strategy
        .record_entry_skip_once(1_200, &skip, EntrySkipReason::EntryPricingBlocked)
        .unwrap();
    entry_strategy
        .pricing
        .observe_reference_current_price(&fast_spot("chainlink", 3_100.5, 1_200));
    let admitted = entry_strategy.entry_submission_decision_at(1_200);
    assert!(
        admitted.instrument_id.is_some(),
        "reset fixture must reach admitted entry order construction"
    );
    let _ = entry_strategy.submit_admitted_entry_decision(1_200, admitted);
    entry_strategy
        .record_entry_skip_once(1_201, &skip, EntrySkipReason::EntryPricingBlocked)
        .unwrap();
    assert_eq!(
        entry_evidence
            .recorded_facts()
            .expect("recorded current evidence must decode")
            .iter()
            .filter(|event| matches!(event, CurrentFact::EntrySkipObservation(_)))
            .count(),
        1,
        "the admitted-entry site no longer resets suppression. A successful submit does not \
         end the market episode, so a skip reason already claimed within it stays claimed -- \
         resetting here is exactly the reclaim the frozen amendment forbids"
    );

    let blocked_evidence = recording_decision_evidence();
    let mut blocked_strategy = rv_clock_domain_amendment_dedupe_strategy(blocked_evidence.clone());
    let blocked = rv_clock_domain_amendment_blocked_decision();
    blocked_strategy
        .record_blocked_entry_strategy_input_snapshot_once(1_200, &blocked)
        .unwrap();
    blocked_strategy
        .submit_admitted_entry_decision(1_200, minimal_entry_submission_decision())
        .expect("non-RV-blocked early return should exercise the existing reset site");
    blocked_strategy
        .record_blocked_entry_strategy_input_snapshot_once(1_201, &blocked)
        .unwrap();
    assert_eq!(
        blocked_evidence
            .recorded_facts()
            .expect("recorded current evidence must decode")
            .iter()
            .filter(|event| matches!(event, CurrentFact::BlockedStrategyInputObservation(_)))
            .count(),
        1,
        "leaving the RV-not-ready condition no longer resets suppression either, for the same \
         reason: the episode is the market, and it did not end"
    );
}

#[test]
fn rv_clock_domain_amendment_entry_skip_writer_failure_marks_seen() {
    let mut strategy = test_strategy_with_fee_provider_and_decision_evidence(
        RecordingFeeProvider::cold(),
        failing_decision_evidence(),
    );
    let decision = minimal_entry_submission_decision();
    assert!(
        strategy
            .record_entry_skip_once(1_200, &decision, EntrySkipReason::EntryPricingBlocked)
            .expect("entry writer errors are swallowed")
    );
    assert!(
        !strategy
            .record_entry_skip_once(1_201, &decision, EntrySkipReason::EntryPricingBlocked)
            .expect("seen entry state should remain suppressed after the swallowed error"),
        "entry-skip failure must preserve mark-before-swallowed-error behavior"
    );
}

#[test]
fn rv_clock_domain_amendment_blocked_snapshot_poisoning_refuses_followup_key_transitions() {
    let evidence = recording_evidence_failing_blocked_attempt(2);
    let mut strategy = rv_clock_domain_amendment_dedupe_strategy(evidence.clone());
    let mut a = rv_clock_domain_amendment_blocked_decision();
    rv_clock_domain_amendment_apply_state(&mut a, RvGateResult::Accepted, None);
    strategy
        .record_blocked_entry_strategy_input_snapshot_once(1_200, &a)
        .expect("A should record");

    let original_market_id = strategy.active.market_id.clone();
    strategy.active.market_id = Some("<KEY_B>".to_string());
    let mut b = a.clone();
    rv_clock_domain_amendment_apply_state(
        &mut b,
        RvGateResult::RejectedStale,
        Some(LocalReceiveMs::new(1_234)),
    );
    strategy
        .record_blocked_entry_strategy_input_snapshot_once(1_201, &b)
        .expect("observation commit failure must remain bounded");

    strategy.active.market_id = original_market_id.clone();
    strategy
        .record_blocked_entry_strategy_input_snapshot_once(1_202, &a)
        .expect("a new key transition must remain non-aborting after sink poisoning");
    assert_eq!(
        evidence.attempts_for(crate::bolt_v3_current_evidence::generated_contract::KnownPurpose::BlockedStrategyInputObservation),
        2,
        "returning to A is not a new semantic state -- A was claimed on the first observation \
         and stays claimed, so the sink is never asked again. The failed B is not retried \
         either: it was claimed before the write, which is what stops a broken sink becoming \
         the flood this suppression exists to prevent"
    );

    strategy.active.market_id = Some("<KEY_B>".to_string());
    strategy
        .record_blocked_entry_strategy_input_snapshot_once(1_203, &b)
        .expect("another key transition must remain non-aborting after sink poisoning");
    assert_eq!(
        evidence.attempts_for(crate::bolt_v3_current_evidence::generated_contract::KnownPurpose::BlockedStrategyInputObservation),
        2,
        "and oscillating back to B adds nothing either: two distinct states, two attempts, \
         however long the oscillation runs"
    );
    let records = evidence
        .recorded_facts()
        .expect("recorded current evidence must decode")
        .into_iter()
        .filter_map(|event| match event {
            CurrentFact::BlockedStrategyInputObservation(snapshot) => Some(snapshot),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        records.len(),
        1,
        "the first commit-indeterminate attempt poisons the sink, so no later transition may append"
    );
    assert_eq!(
        blocked_rv_metadata(&records[0]),
        (RvGateResult::Accepted, None)
    );
}

#[test]
fn rv_clock_domain_amendment_blocked_snapshot_same_key_failure_marks_bit_seen_without_retry() {
    let evidence = recording_evidence_failing_blocked_attempt(2);
    let mut strategy = rv_clock_domain_amendment_dedupe_strategy(evidence.clone());
    let mut decision = rv_clock_domain_amendment_blocked_decision();

    rv_clock_domain_amendment_apply_state(
        &mut decision,
        RvGateResult::Accepted,
        Some(LocalReceiveMs::new(1_200)),
    );
    strategy
        .record_blocked_entry_strategy_input_snapshot_once(1_200, &decision)
        .expect("the first bit on the key should commit");

    rv_clock_domain_amendment_apply_state(
        &mut decision,
        RvGateResult::RejectedStale,
        Some(LocalReceiveMs::new(1_234)),
    );
    strategy
        .record_blocked_entry_strategy_input_snapshot_once(1_201, &decision)
        .expect("observation commit failure must remain bounded");

    rv_clock_domain_amendment_apply_state(
        &mut decision,
        RvGateResult::Accepted,
        Some(LocalReceiveMs::new(1_200)),
    );
    strategy
        .record_blocked_entry_strategy_input_snapshot_once(1_202, &decision)
        .expect("the committed bit should remain suppressed");
    assert_eq!(
        evidence.attempts_for(crate::bolt_v3_current_evidence::generated_contract::KnownPurpose::BlockedStrategyInputObservation),
        2,
        "a failed new bit must neither clear the committed bit nor reach the writer again"
    );

    rv_clock_domain_amendment_apply_state(
        &mut decision,
        RvGateResult::RejectedStale,
        Some(LocalReceiveMs::new(1_234)),
    );
    strategy
        .record_blocked_entry_strategy_input_snapshot_once(1_203, &decision)
        .expect("the failed new bit must remain seen and suppressed on the same key");
    assert_eq!(
        evidence.attempts_for(crate::bolt_v3_current_evidence::generated_contract::KnownPurpose::BlockedStrategyInputObservation),
        2,
        "an indeterminate observation fact must never be retried"
    );

    let recorded_states = evidence
        .recorded_facts()
        .expect("recorded current evidence must decode")
        .into_iter()
        .filter_map(|event| match event {
            CurrentFact::BlockedStrategyInputObservation(snapshot) => {
                Some(blocked_rv_metadata(&snapshot))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        recorded_states,
        vec![(RvGateResult::Accepted, Some(1_200))],
        "only the fact committed before poisoning may be readable"
    );
}

#[test]
fn rv_clock_domain_amendment_blocked_snapshot_payload_failure_is_claimed_without_retry() {
    let evidence = recording_decision_evidence();
    let mut strategy = rv_clock_domain_amendment_dedupe_strategy(evidence.clone());
    let decision = rv_clock_domain_amendment_blocked_decision();
    let episode = strategy
        .evidence_episode_id()
        .expect("the dedupe fixture must bind a market episode");

    strategy.active.last_reference_ts_ms = None;
    strategy
        .record_blocked_entry_strategy_input_snapshot_once(1_200, &decision)
        .expect("telemetry payload failure must not abort the strategy callback");
    strategy.active.last_reference_ts_ms = Some(1_201);
    strategy
        .record_blocked_entry_strategy_input_snapshot_once(1_201, &decision)
        .expect("a failed payload must remain claimed rather than retrying");

    assert_eq!(
        strategy
            .blocked_strategy_input_novelty
            .seen_state_count(&episode),
        1,
        "the registered state must be claimed before fallible payload construction"
    );
    assert_eq!(
        evidence.attempts_for(
            crate::bolt_v3_current_evidence::generated_contract::KnownPurpose::
                BlockedStrategyInputObservation,
        ),
        0,
        "payload construction failure must never reach the evidence writer, including on a later tick"
    );
}
