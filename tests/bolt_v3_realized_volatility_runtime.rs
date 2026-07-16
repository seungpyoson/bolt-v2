use std::collections::BTreeMap;

use bolt_v2::{
    bolt_v3_config::load_bolt_v3_config,
    bolt_v3_realized_volatility::{
        RealizedVolAggregation, RealizedVolBlockReason, RealizedVolEngineConfig,
        RealizedVolObservation, RealizedVolSampleKind, RealizedVolSourceClass,
        RealizedVolSourceConfig, RealizedVolSourceRejectReason, RealizedVolSourceStatus,
    },
    bolt_v3_realized_volatility_runtime::RealizedVolSurfaceRuntime,
    bolt_v3_timestamp_domain::NtStrategyClockMs,
    nautilus_source_capabilities::NAUTILUS_SOURCE_CAPABILITIES,
};
use nautilus_core::UnixNanos;
use nautilus_model::{
    data::QuoteTick,
    identifiers::InstrumentId,
    types::{Price, Quantity},
};
use serde::Deserialize;

const SURFACE_A: &str = "<SURFACE_A>";
const SURFACE_B: &str = "<SURFACE_B>";
const SOURCE_A: &str = "<SOURCE_A>";
const SOURCE_B: &str = "<SOURCE_B>";
const NANOS_PER_MILLI: u64 = 1_000_000;

fn source(source_id: &str, instrument_id: &str) -> RealizedVolSourceConfig {
    RealizedVolSourceConfig {
        source_id: source_id.to_string(),
        data_client_id: "<DATA_CLIENT_ID>".to_string(),
        instrument_id: instrument_id.to_string(),
        source_class: RealizedVolSourceClass::SpotQuote,
        sample_kind: RealizedVolSampleKind::Midpoint,
        enabled: true,
        counts_toward_quorum: true,
        canonical_base_asset: "<BASE_ASSET>".to_string(),
        canonical_quote_asset: "<QUOTE_ASSET>".to_string(),
    }
}

fn config(surface_id: &str, source_id: &str, instrument_id: &str) -> RealizedVolEngineConfig {
    RealizedVolEngineConfig {
        surface_id: surface_id.to_string(),
        window_ms: 4_000,
        sampling_interval_ms: 1_000,
        min_ready_sources: 1,
        max_source_age_ms: 500,
        max_inter_sample_gap_ms: 2_000,
        min_coverage_ratio: 0.75,
        max_cross_source_dispersion: 0.50,
        seconds_per_annum: 31_536_000.0,
        aggregation: RealizedVolAggregation::UpperQuantile { quantile: 1.0 },
        estimator: bolt_v2::bolt_v3_realized_volatility::RealizedVolEstimatorConfig::measured(),
        sources: vec![source(source_id, instrument_id)],
    }
}

fn observation(source_id: &str, price: f64, ts_ms: u64) -> RealizedVolObservation {
    RealizedVolObservation {
        source_id: source_id.to_string(),
        source_class: RealizedVolSourceClass::SpotQuote,
        sample_kind: RealizedVolSampleKind::Midpoint,
        price,
        event_ts_ms: ts_ms,
        recv_ts_ms: ts_ms,
    }
}

fn strategy_clock_ms(value: u64) -> NtStrategyClockMs {
    NtStrategyClockMs::new(value)
}

fn quote_tick_with_receive_ms(
    instrument_id: &str,
    bid: f64,
    ask: f64,
    event_ts_ms: u64,
    recv_ts_ms: u64,
) -> QuoteTick {
    QuoteTick::new_checked(
        InstrumentId::from(instrument_id),
        Price::new(bid, 2),
        Price::new(ask, 2),
        Quantity::new(1.0, 0),
        Quantity::new(1.0, 0),
        UnixNanos::from(event_ts_ms.saturating_mul(NANOS_PER_MILLI)),
        UnixNanos::from(recv_ts_ms.saturating_mul(NANOS_PER_MILLI)),
    )
    .expect("test quote tick should be valid")
}

#[derive(Debug, Deserialize)]
struct QuoteReplayFixture {
    quotes: Vec<QuoteReplayTick>,
}

#[derive(Debug, Deserialize)]
struct QuoteReplayTick {
    bid: String,
    ask: String,
    event_ts_ms: u64,
    recv_ts_ms: u64,
}

impl QuoteReplayTick {
    fn bid(&self) -> f64 {
        self.bid.parse().expect("fixture bid should parse")
    }

    fn ask(&self) -> f64 {
        self.ask.parse().expect("fixture ask should parse")
    }
}

fn event_clock_replay_fixture() -> QuoteReplayFixture {
    toml::from_str(include_str!("fixtures/bolt_v3/rv_event_clock_replay.toml"))
        .expect("event-clock RV replay fixture should parse")
}

#[test]
fn official_pin_capabilities_fail_closed_for_binance_sbe_new_risk_quorum() {
    assert_eq!(
        NAUTILUS_SOURCE_CAPABILITIES.git,
        "https://github.com/nautechsystems/nautilus_trader.git"
    );
    assert_eq!(
        NAUTILUS_SOURCE_CAPABILITIES.revision,
        "8160730c7c550480b0a439fb11086a4c4de15f0b"
    );
    assert!(!NAUTILUS_SOURCE_CAPABILITIES.binance_spot_sbe_schema_3_5);
    assert!(!NAUTILUS_SOURCE_CAPABILITIES.binance_spot_sbe_adapter_receive_clock);
    assert!(!NAUTILUS_SOURCE_CAPABILITIES.binance_spot_sbe_new_risk_quorum);
}

#[test]
fn unavailable_binance_sbe_capability_keeps_runtime_reachable_but_cannot_ready_quorum() {
    let mut loaded = load_bolt_v3_config(&super::support::repo_path("config/root.toml"))
        .expect("production config should load");
    let surfaces = loaded
        .root
        .realized_volatility_surfaces
        .as_mut()
        .expect("production config should declare RV surfaces");
    let surface = surfaces
        .get_mut("btc_usdt_midpoint_rv")
        .expect("production config should declare the BTC/USDT RV surface");
    surface
        .sources
        .retain(|source| source.data_client_id.as_str() == "binance_spot_data");

    let mut runtime = RealizedVolSurfaceRuntime::from_loaded_config(&loaded)
        .expect("missing provider capabilities must not prevent runtime construction");
    assert!(
        runtime
            .subscription_requests_for_surface("btc_usdt_midpoint_rv")
            .is_empty(),
        "unavailable Binance SBE must not create an ingestion route"
    );
    assert!(!runtime.observe(RealizedVolObservation {
        source_id: "binance_btc_usdt_midpoint".to_string(),
        source_class: RealizedVolSourceClass::SpotQuote,
        sample_kind: RealizedVolSampleKind::Midpoint,
        price: 100_000.0,
        event_ts_ms: 1_000,
        recv_ts_ms: 1_001,
    }));

    let snapshot = runtime
        .refresh_surface_at("btc_usdt_midpoint_rv", strategy_clock_ms(1_001))
        .expect("configured surface should remain refreshable");
    assert!(!snapshot.ready);
    assert_eq!(snapshot.ready_realized_vol(), None);
    assert!(
        snapshot
            .blocked_reasons
            .contains(&RealizedVolBlockReason::QuorumNotReady)
    );
    let diagnostic = snapshot
        .source_diagnostics
        .iter()
        .find(|diagnostic| diagnostic.source_id == "binance_btc_usdt_midpoint")
        .expect("unavailable source should remain visible in diagnostics");
    assert!(!diagnostic.counts_toward_quorum);
    assert_eq!(diagnostic.status, RealizedVolSourceStatus::DiagnosticOnly);
    assert_eq!(
        diagnostic.block_reason,
        Some(RealizedVolBlockReason::ProviderCapabilityUnavailable)
    );
}

#[test]
fn runtime_builds_all_surfaces_from_config_map() {
    let runtime = RealizedVolSurfaceRuntime::from_configs(BTreeMap::from([
        (
            SURFACE_A.to_string(),
            config(SURFACE_A, SOURCE_A, "<INSTRUMENT_A>.<DATA_CLIENT_ID>"),
        ),
        (
            SURFACE_B.to_string(),
            config(SURFACE_B, SOURCE_B, "<INSTRUMENT_B>.<DATA_CLIENT_ID>"),
        ),
    ]))
    .expect("runtime should build all configured surfaces");

    assert_eq!(
        runtime.surface_ids(),
        vec![SURFACE_A.to_string(), SURFACE_B.to_string()]
    );
}

#[test]
fn runtime_publishes_snapshot_by_surface_id_for_multiple_consumers() {
    let mut runtime = RealizedVolSurfaceRuntime::from_configs(BTreeMap::from([(
        SURFACE_A.to_string(),
        config(SURFACE_A, SOURCE_A, "<INSTRUMENT_A>.<DATA_CLIENT_ID>"),
    )]))
    .expect("runtime should build");

    for (index, price) in [100.0, 101.0, 102.0, 103.0].iter().enumerate() {
        assert!(runtime.observe(observation(SOURCE_A, *price, (index as u64 + 1) * 1_000)));
    }
    let snapshot = runtime
        .refresh_surface_at(SURFACE_A, strategy_clock_ms(4_000))
        .expect("configured surface should publish a snapshot");

    let pricing_consumer = runtime
        .snapshot(SURFACE_A)
        .expect("pricing consumer should read latest snapshot");
    let monitoring_consumer = runtime
        .snapshot(SURFACE_A)
        .expect("monitoring consumer should read same latest snapshot");

    assert!(snapshot.ready);
    assert_eq!(pricing_consumer, monitoring_consumer);
    assert_eq!(pricing_consumer.surface_id, SURFACE_A);
}

#[test]
fn routed_quote_replay_uses_event_clock_for_rv_windows() {
    let instrument_id = "<INSTRUMENT_A>.<DATA_CLIENT_ID>";
    let fixture = event_clock_replay_fixture();
    let mut runtime = RealizedVolSurfaceRuntime::from_configs(BTreeMap::from([(
        SURFACE_A.to_string(),
        config(SURFACE_A, SOURCE_A, instrument_id),
    )]))
    .expect("runtime should build");

    assert_eq!(fixture.quotes.len(), 4);
    for quote in &fixture.quotes {
        let snapshots = runtime.observe_quote(&quote_tick_with_receive_ms(
            instrument_id,
            quote.bid(),
            quote.ask(),
            quote.event_ts_ms,
            quote.recv_ts_ms,
        ));
        let latest = snapshots
            .last()
            .expect("routed quote should publish a surface snapshot");
        assert_eq!(latest.as_of_ms, quote.event_ts_ms);
    }

    let snapshot = runtime
        .snapshot(SURFACE_A)
        .expect("event-clock replay should publish the latest snapshot");

    assert!(snapshot.ready);
    assert_eq!(snapshot.as_of_ms, 4_000);
    assert_eq!(snapshot.source_diagnostics[0].raw_sample_count, 4);
    assert_eq!(snapshot.source_diagnostics[0].grid_sample_count, 4);
    assert!(
        snapshot.annualized_realized_vol_decimal.is_some(),
        "event-clock quote sequence should populate realized volatility"
    );
}

#[test]
fn routed_quote_preserves_event_and_receive_timestamp_domains() {
    let instrument_id = "<INSTRUMENT_A>.<DATA_CLIENT_ID>";
    let mut runtime = RealizedVolSurfaceRuntime::from_configs(BTreeMap::from([(
        SURFACE_A.to_string(),
        config(SURFACE_A, SOURCE_A, instrument_id),
    )]))
    .expect("runtime should build");

    for (index, (bid, ask)) in [
        (99.0, 101.0),
        (100.0, 102.0),
        (101.0, 103.0),
        (102.0, 104.0),
    ]
    .iter()
    .enumerate()
    {
        let event_ts_ms = (index as u64 + 1) * 1_000;
        let receive_ts_ms = event_ts_ms - 750;
        runtime.observe_quote(&quote_tick_with_receive_ms(
            instrument_id,
            *bid,
            *ask,
            event_ts_ms,
            receive_ts_ms,
        ));
    }

    let snapshot = runtime
        .snapshot(SURFACE_A)
        .expect("routed quotes should publish the surface");

    assert!(snapshot.ready);
    assert_eq!(
        snapshot.as_of_ms, 4_000,
        "surface event time must come from QuoteTick.ts_event"
    );
    assert_eq!(
        snapshot
            .latest_accepted_receive_ms
            .map(|stamp| stamp.value()),
        Some(3_250),
        "surface receive watermark must come from QuoteTick.ts_init"
    );
}

#[test]
fn strategy_clock_refresh_does_not_contaminate_event_domain_publication() {
    let instrument_id = "<INSTRUMENT_A>.<DATA_CLIENT_ID>";
    let mut runtime = RealizedVolSurfaceRuntime::from_configs(BTreeMap::from([(
        SURFACE_A.to_string(),
        config(SURFACE_A, SOURCE_A, instrument_id),
    )]))
    .expect("runtime should build");

    for (index, (bid, ask)) in [
        (99.0, 101.0),
        (100.0, 102.0),
        (101.0, 103.0),
        (102.0, 104.0),
    ]
    .iter()
    .enumerate()
    {
        let event_ts_ms = (index as u64 + 1) * 1_000;
        let snapshots = runtime.observe_quote(&quote_tick_with_receive_ms(
            instrument_id,
            *bid,
            *ask,
            event_ts_ms,
            event_ts_ms,
        ));
        assert_eq!(
            snapshots
                .last()
                .expect("routed quote should publish a surface snapshot")
                .as_of_ms,
            event_ts_ms
        );
    }

    let refreshed = runtime
        .refresh_surface_at(SURFACE_A, strategy_clock_ms(4_501))
        .expect("strategy-clock refresh should publish diagnostics");
    assert_eq!(
        refreshed.as_of_ms, 4_000,
        "strategy-clock refresh must not become event-domain as_of evidence"
    );

    let later_event_snapshots = runtime.observe_quote(&quote_tick_with_receive_ms(
        instrument_id,
        103.0,
        105.0,
        4_250,
        4_501,
    ));
    assert_eq!(
        later_event_snapshots
            .last()
            .expect("later event-domain quote should publish")
            .as_of_ms,
        4_250,
        "later event-domain publish must not max against the prior strategy-clock refresh"
    );
}

#[test]
fn routed_quote_rejection_refreshes_diagnostics_without_regressing_snapshot_clock() {
    let instrument_id = "<INSTRUMENT_A>.<DATA_CLIENT_ID>";
    let mut runtime = RealizedVolSurfaceRuntime::from_configs(BTreeMap::from([(
        SURFACE_A.to_string(),
        config(SURFACE_A, SOURCE_A, instrument_id),
    )]))
    .expect("runtime should build");

    for (index, (bid, ask)) in [
        (99.0, 101.0),
        (100.0, 102.0),
        (101.0, 103.0),
        (102.0, 104.0),
    ]
    .iter()
    .enumerate()
    {
        let event_ts_ms = (index as u64 + 1) * 1_000;
        let snapshots = runtime.observe_quote(&quote_tick_with_receive_ms(
            instrument_id,
            *bid,
            *ask,
            event_ts_ms,
            event_ts_ms,
        ));
        assert_eq!(
            snapshots
                .last()
                .expect("routed quote should publish a surface snapshot")
                .as_of_ms,
            event_ts_ms
        );
    }

    let published = runtime
        .snapshot(SURFACE_A)
        .expect("event-clock quote sequence should publish a snapshot");
    assert!(published.ready);
    assert_eq!(published.as_of_ms, 4_000);
    assert_eq!(published.source_diagnostics[0].last_rejected_reason, None);

    let late_snapshots = runtime.observe_quote(&quote_tick_with_receive_ms(
        instrument_id,
        103.0,
        105.0,
        3_500,
        5_000,
    ));
    let latest = late_snapshots
        .last()
        .expect("routed rejection should republish diagnostics");
    assert_eq!(latest.as_of_ms, 4_000);

    let diagnostic = latest
        .source_diagnostics
        .iter()
        .find(|diagnostic| diagnostic.source_id == SOURCE_A)
        .expect("configured source should remain diagnostic-visible");
    assert_eq!(
        diagnostic.last_rejected_reason,
        Some(RealizedVolSourceRejectReason::EventTimeRegression)
    );
    assert_eq!(
        diagnostic
            .rejection_counters
            .get(&RealizedVolSourceRejectReason::EventTimeRegression),
        Some(&1)
    );
}

#[test]
fn routed_cross_source_update_recomputes_at_surface_clock_when_event_time_skews_lower() {
    let instrument_a = "<INSTRUMENT_A>.<DATA_CLIENT_ID>";
    let instrument_b = "<INSTRUMENT_B>.<DATA_CLIENT_ID>";
    let mut surface_config = config(SURFACE_A, SOURCE_A, instrument_a);
    surface_config.sources.push(source(SOURCE_B, instrument_b));
    let mut runtime = RealizedVolSurfaceRuntime::from_configs(BTreeMap::from([(
        SURFACE_A.to_string(),
        surface_config,
    )]))
    .expect("runtime should build");

    for (index, (bid, ask)) in [
        (99.0, 101.0),
        (100.0, 102.0),
        (101.0, 103.0),
        (102.0, 104.0),
    ]
    .iter()
    .enumerate()
    {
        let event_ts_ms = (index as u64 + 1) * 1_000;
        let snapshots = runtime.observe_quote(&quote_tick_with_receive_ms(
            instrument_a,
            *bid,
            *ask,
            event_ts_ms,
            event_ts_ms,
        ));
        assert_eq!(
            snapshots
                .last()
                .expect("source A quote should publish a surface snapshot")
                .as_of_ms,
            event_ts_ms
        );
    }

    let published = runtime
        .snapshot(SURFACE_A)
        .expect("source A quote sequence should publish a snapshot");
    assert_eq!(published.as_of_ms, 4_000);
    let initial_source_b = published
        .source_diagnostics
        .iter()
        .find(|diagnostic| diagnostic.source_id == SOURCE_B)
        .expect("source B should be diagnostic-visible before samples arrive");
    assert_eq!(initial_source_b.raw_sample_count, 0);

    let skewed_snapshots = runtime.observe_quote(&quote_tick_with_receive_ms(
        instrument_b,
        200.0,
        202.0,
        3_500,
        3_500,
    ));
    let latest = skewed_snapshots
        .last()
        .expect("accepted skewed source update should publish diagnostics");
    assert_eq!(latest.as_of_ms, 4_000);
    assert_eq!(
        latest.latest_accepted_receive_ms.map(|stamp| stamp.value()),
        Some(4_000),
        "the ready surface watermark follows accepted quorum contributors"
    );
    let source_b = latest
        .source_diagnostics
        .iter()
        .find(|diagnostic| diagnostic.source_id == SOURCE_B)
        .expect("source B should remain diagnostic-visible");
    assert_eq!(source_b.raw_sample_count, 1);
}

#[test]
fn rejected_routed_observation_does_not_advance_the_surface_watermark() {
    let instrument_a = "<INSTRUMENT_A>.<DATA_CLIENT_ID>";
    let instrument_b = "<INSTRUMENT_B>.<DATA_CLIENT_ID>";
    let mut surface_config = config(SURFACE_A, SOURCE_A, instrument_a);
    let mut disabled_source = source(SOURCE_B, instrument_b);
    disabled_source.enabled = false;
    disabled_source.counts_toward_quorum = false;
    surface_config.sources.push(disabled_source);
    let mut runtime = RealizedVolSurfaceRuntime::from_configs(BTreeMap::from([(
        SURFACE_A.to_string(),
        surface_config,
    )]))
    .expect("runtime should build");

    for (index, (bid, ask)) in [
        (99.0, 101.0),
        (100.0, 102.0),
        (101.0, 103.0),
        (102.0, 104.0),
    ]
    .iter()
    .enumerate()
    {
        let ts_ms = (index as u64 + 1) * 1_000;
        runtime.observe_quote(&quote_tick_with_receive_ms(
            instrument_a,
            *bid,
            *ask,
            ts_ms,
            ts_ms,
        ));
    }
    assert_eq!(
        runtime
            .snapshot(SURFACE_A)
            .map(|snapshot| snapshot.as_of_ms),
        Some(4_000)
    );

    let snapshots = runtime.observe_quote(&quote_tick_with_receive_ms(
        instrument_b,
        200.0,
        202.0,
        50_000,
        50_000,
    ));
    let latest = snapshots
        .last()
        .expect("rejected routed observations should republish diagnostics");

    assert_eq!(
        latest.as_of_ms, 4_000,
        "a rejected observation must not refresh the causal surface watermark"
    );
    assert_eq!(
        latest.latest_accepted_receive_ms.map(|stamp| stamp.value()),
        Some(4_000),
        "a rejected observation must not refresh the accepted receive watermark"
    );
    let diagnostic = latest
        .source_diagnostics
        .iter()
        .find(|diagnostic| diagnostic.source_id == SOURCE_B)
        .expect("disabled source rejection should remain diagnostic-visible");
    assert_eq!(
        diagnostic.last_rejected_reason,
        Some(RealizedVolSourceRejectReason::DisabledSource)
    );
}

#[test]
fn runtime_refresh_ignores_stale_and_equal_explicit_refresh_timestamps() {
    let mut runtime = RealizedVolSurfaceRuntime::from_configs(BTreeMap::from([(
        SURFACE_A.to_string(),
        config(SURFACE_A, SOURCE_A, "<INSTRUMENT_A>.<DATA_CLIENT_ID>"),
    )]))
    .expect("runtime should build");

    for (index, price) in [100.0, 101.0, 102.0, 103.0].iter().enumerate() {
        assert!(runtime.observe(observation(SOURCE_A, *price, (index as u64 + 1) * 1_000)));
    }
    let first = runtime
        .refresh_surface_at(SURFACE_A, strategy_clock_ms(4_000))
        .expect("first refresh should publish");
    let equal = runtime
        .refresh_surface_at(SURFACE_A, strategy_clock_ms(4_000))
        .expect("equal refresh should return current snapshot");
    let stale = runtime
        .refresh_surface_at(SURFACE_A, strategy_clock_ms(3_000))
        .expect("stale refresh should return current snapshot");

    assert_eq!(first.as_of_ms, 4_000);
    assert_eq!(equal, first);
    assert_eq!(stale, first);
    assert_eq!(runtime.snapshot(SURFACE_A), Some(first));
}

#[test]
fn runtime_direct_observe_wrong_sample_kind_rejects_without_republishing_snapshot() {
    let mut runtime = RealizedVolSurfaceRuntime::from_configs(BTreeMap::from([(
        SURFACE_A.to_string(),
        config(SURFACE_A, SOURCE_A, "<INSTRUMENT_A>.<DATA_CLIENT_ID>"),
    )]))
    .expect("runtime should build");

    for (index, price) in [100.0, 101.0, 102.0, 103.0].iter().enumerate() {
        assert!(runtime.observe(observation(SOURCE_A, *price, (index as u64 + 1) * 1_000)));
    }
    let published = runtime
        .refresh_surface_at(SURFACE_A, strategy_clock_ms(4_000))
        .expect("ready snapshot should publish");

    assert!(!runtime.observe(RealizedVolObservation {
        sample_kind: RealizedVolSampleKind::Trade,
        price: 104.0,
        event_ts_ms: 5_000,
        recv_ts_ms: 5_000,
        ..observation(SOURCE_A, 104.0, 5_000)
    }));
    assert_eq!(
        runtime.snapshot(SURFACE_A),
        Some(published),
        "direct rejected observations must not publish a new snapshot"
    );

    let refreshed = runtime
        .refresh_surface_at(SURFACE_A, strategy_clock_ms(5_000))
        .expect("explicit refresh should expose rejection diagnostics");
    let diagnostic = refreshed
        .source_diagnostics
        .iter()
        .find(|diagnostic| diagnostic.source_id == SOURCE_A)
        .expect("configured source should remain diagnostic-visible");
    assert_eq!(
        diagnostic.last_rejected_reason,
        Some(RealizedVolSourceRejectReason::SampleKindMismatch)
    );
    assert_eq!(
        diagnostic
            .rejection_counters
            .get(&RealizedVolSourceRejectReason::SampleKindMismatch),
        Some(&1)
    );
}

#[test]
fn runtime_direct_observe_unknown_source_drops_at_boundary_without_polluting_surfaces() {
    let mut runtime = RealizedVolSurfaceRuntime::from_configs(BTreeMap::from([
        (
            SURFACE_A.to_string(),
            config(SURFACE_A, SOURCE_A, "<INSTRUMENT_A>.<DATA_CLIENT_ID>"),
        ),
        (
            SURFACE_B.to_string(),
            config(SURFACE_B, SOURCE_B, "<INSTRUMENT_B>.<DATA_CLIENT_ID>"),
        ),
    ]))
    .expect("runtime should build");

    assert!(!runtime.observe(observation("<UNKNOWN_SOURCE>", 100.0, 1_000)));

    for surface_id in [SURFACE_A, SURFACE_B] {
        let snapshot = runtime
            .refresh_surface_at(surface_id, strategy_clock_ms(1_000))
            .expect("configured surface should publish diagnostics");
        assert!(
            snapshot.unknown_source_rejections.is_empty(),
            "unknown direct observations should be dropped before reaching surface {surface_id}"
        );
    }
}

#[test]
fn runtime_generic_observe_fans_out_duplicate_source_ids_across_surfaces() {
    let mut runtime = RealizedVolSurfaceRuntime::from_configs(BTreeMap::from([
        (
            SURFACE_A.to_string(),
            config(SURFACE_A, SOURCE_A, "<INSTRUMENT_A>.<DATA_CLIENT_ID>"),
        ),
        (
            SURFACE_B.to_string(),
            config(SURFACE_B, SOURCE_A, "<INSTRUMENT_B>.<DATA_CLIENT_ID>"),
        ),
    ]))
    .expect("runtime should build");

    for (index, price) in [100.0, 101.0, 102.0, 103.0].iter().enumerate() {
        assert!(runtime.observe(observation(SOURCE_A, *price, (index as u64 + 1) * 1_000)));
    }

    let surface_a = runtime
        .refresh_surface_at(SURFACE_A, strategy_clock_ms(4_000))
        .expect("surface A should publish");
    let surface_b = runtime
        .refresh_surface_at(SURFACE_B, strategy_clock_ms(4_000))
        .expect("surface B should publish");

    assert!(surface_a.ready);
    assert!(surface_b.ready);
}

#[test]
fn runtime_rejects_mark_sources_until_subscription_routing_exists() {
    let mut cfg = config(SURFACE_A, SOURCE_A, "<INSTRUMENT_A>.<DATA_CLIENT_ID>");
    cfg.sources[0].source_class = RealizedVolSourceClass::Mark;
    cfg.sources[0].sample_kind = RealizedVolSampleKind::Mark;

    let error =
        RealizedVolSurfaceRuntime::from_configs(BTreeMap::from([(SURFACE_A.to_string(), cfg)]))
            .expect_err("mark routing should fail closed until runtime support exists");

    assert!(error.contains("mark"));
}

#[test]
fn surface_scoped_subscription_requests_partition_by_configured_surface() {
    let runtime = RealizedVolSurfaceRuntime::from_configs(BTreeMap::from([
        (
            SURFACE_A.to_string(),
            config(SURFACE_A, SOURCE_A, "<INSTRUMENT_A>.<DATA_CLIENT_ID>"),
        ),
        (
            SURFACE_B.to_string(),
            config(SURFACE_B, SOURCE_B, "<INSTRUMENT_B>.<DATA_CLIENT_ID>"),
        ),
    ]))
    .expect("runtime should build both surfaces");

    let surface_a_quote_ids = runtime
        .quote_subscription_requests_for_surface(SURFACE_A)
        .into_iter()
        .map(|(instrument_id, _)| instrument_id)
        .collect::<Vec<_>>();
    let surface_b_quote_ids = runtime
        .quote_subscription_requests_for_surface(SURFACE_B)
        .into_iter()
        .map(|(instrument_id, _)| instrument_id)
        .collect::<Vec<_>>();

    // Each surface exposes only its own source.
    assert_eq!(surface_a_quote_ids.len(), 1);
    assert_eq!(surface_b_quote_ids.len(), 1);
    // Disjoint: a strategy on surface A must not subscribe surface B's instrument.
    assert_ne!(surface_a_quote_ids[0], surface_b_quote_ids[0]);

    // An unknown surface yields no subscriptions (fail-closed: pricing stays NotReady).
    assert!(
        runtime
            .quote_subscription_requests_for_surface("<UNKNOWN_SURFACE>")
            .is_empty()
    );
}

#[test]
fn global_subscription_requests_dedupe_across_surfaces_sharing_an_instrument() {
    // Two surfaces share the same (instrument_id, data_client_id, kind). The per-surface
    // accessors keep each surface's request; the global (audit) accessor must dedupe so the
    // historical fanout count is unchanged.
    let shared_instrument = "<SHARED_INSTRUMENT>.<DATA_CLIENT_ID>";
    let runtime = RealizedVolSurfaceRuntime::from_configs(BTreeMap::from([
        (
            SURFACE_A.to_string(),
            config(SURFACE_A, SOURCE_A, shared_instrument),
        ),
        (
            SURFACE_B.to_string(),
            config(SURFACE_B, SOURCE_B, shared_instrument),
        ),
    ]))
    .expect("runtime should build both surfaces");

    // Per-surface: each surface owns one request.
    assert_eq!(
        runtime.subscription_requests_for_surface(SURFACE_A).len(),
        1
    );
    assert_eq!(
        runtime.subscription_requests_for_surface(SURFACE_B).len(),
        1
    );

    // Global audit view: deduped union collapses the shared instrument to a single request.
    assert_eq!(runtime.subscription_requests().len(), 1);
}
