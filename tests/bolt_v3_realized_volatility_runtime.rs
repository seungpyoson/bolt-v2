use std::collections::BTreeMap;

use bolt_v2::{
    bolt_v3_realized_volatility::{
        RealizedVolAggregation, RealizedVolEngineConfig, RealizedVolObservation,
        RealizedVolSampleKind, RealizedVolSourceClass, RealizedVolSourceConfig,
    },
    bolt_v3_realized_volatility_runtime::RealizedVolSurfaceRuntime,
};

const SURFACE_A: &str = "<SURFACE_A>";
const SURFACE_B: &str = "<SURFACE_B>";
const SOURCE_A: &str = "<SOURCE_A>";
const SOURCE_B: &str = "<SOURCE_B>";

fn source(source_id: &str, instrument_id: &str) -> RealizedVolSourceConfig {
    RealizedVolSourceConfig {
        source_id: source_id.to_string(),
        data_client_id: "<DATA_CLIENT_ID>".to_string(),
        instrument_id: instrument_id.to_string(),
        source_class: RealizedVolSourceClass::SpotQuote,
        sample_kind: RealizedVolSampleKind::Midpoint,
        enabled: true,
        counts_toward_quorum: true,
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
        max_event_receive_lag_ms: 250,
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
        .refresh_surface_at(SURFACE_A, 4_000)
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
        .refresh_surface_at(SURFACE_A, 4_000)
        .expect("surface A should publish");
    let surface_b = runtime
        .refresh_surface_at(SURFACE_B, 4_000)
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
