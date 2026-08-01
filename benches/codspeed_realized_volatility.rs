mod support;

use std::sync::OnceLock;

use bolt_v2::bolt_v3_realized_volatility::{
    RealizedVolAggregation, RealizedVolEngine, RealizedVolEngineConfig, RealizedVolEstimatorConfig,
    RealizedVolObservation, RealizedVolSampleKind, RealizedVolSourceClass, RealizedVolSourceConfig,
};
use serde::Deserialize;

#[derive(Deserialize)]
struct FixtureFile {
    realized_volatility: RealizedVolFixture,
}

#[derive(Deserialize)]
struct RealizedVolFixture {
    surface_id: String,
    window_ms: u64,
    sampling_interval_ms: u64,
    min_ready_sources: usize,
    max_source_age_ms: u64,
    max_inter_sample_gap_ms: u64,
    min_coverage_ratio: f64,
    max_cross_source_dispersion: f64,
    seconds_per_annum: f64,
    aggregation_upper_quantile: f64,
    snapshot_as_of_ms: u64,
    sources: Vec<SourceFixture>,
    observations: Vec<ObservationFixture>,
}

#[derive(Deserialize)]
struct SourceFixture {
    source_id: String,
    data_client_id: String,
    instrument_id: String,
    source_class: String,
    sample_kind: String,
    enabled: bool,
    counts_toward_quorum: bool,
    canonical_base_asset: String,
    canonical_quote_asset: String,
}

#[derive(Deserialize)]
struct ObservationFixture {
    price: f64,
    event_ts_ms: u64,
    recv_ts_ms: u64,
}

struct ObserveCase {
    engine: RealizedVolEngine,
    observations: Vec<RealizedVolObservation>,
    snapshot_as_of_ms: u64,
}

fn main() {
    divan::main();
}

fn fixture() -> &'static RealizedVolFixture {
    static FIXTURE: OnceLock<RealizedVolFixture> = OnceLock::new();
    FIXTURE.get_or_init(|| support::decode_fixtures::<FixtureFile>().realized_volatility)
}

#[divan::bench]
fn construct_engine(bencher: divan::Bencher<'_, '_>) {
    bencher
        .with_inputs(|| engine_config(fixture()))
        .bench_values(|config| {
            divan::black_box(RealizedVolEngine::from_config(divan::black_box(config)))
        });
}

#[divan::bench]
fn observe_and_snapshot(bencher: divan::Bencher<'_, '_>) {
    bencher
        .with_inputs(|| observe_case(fixture()))
        .bench_values(|mut case| {
            for observation in case.observations {
                divan::black_box(case.engine.observe(divan::black_box(observation)));
            }
            divan::black_box(
                case.engine
                    .snapshot_at(divan::black_box(case.snapshot_as_of_ms)),
            )
        });
}

fn engine_config(fixture: &RealizedVolFixture) -> RealizedVolEngineConfig {
    RealizedVolEngineConfig {
        surface_id: fixture.surface_id.clone(),
        window_ms: fixture.window_ms,
        sampling_interval_ms: fixture.sampling_interval_ms,
        min_ready_sources: fixture.min_ready_sources,
        max_source_age_ms: fixture.max_source_age_ms,
        max_inter_sample_gap_ms: fixture.max_inter_sample_gap_ms,
        min_coverage_ratio: fixture.min_coverage_ratio,
        max_cross_source_dispersion: fixture.max_cross_source_dispersion,
        seconds_per_annum: fixture.seconds_per_annum,
        aggregation: RealizedVolAggregation::UpperQuantile {
            quantile: fixture.aggregation_upper_quantile,
        },
        estimator: RealizedVolEstimatorConfig::measured(),
        sources: fixture.sources.iter().map(source_config).collect(),
    }
}

fn source_config(source: &SourceFixture) -> RealizedVolSourceConfig {
    RealizedVolSourceConfig {
        source_id: source.source_id.clone(),
        data_client_id: source.data_client_id.clone(),
        instrument_id: source.instrument_id.clone(),
        source_class: source_class(&source.source_class),
        sample_kind: sample_kind(&source.sample_kind),
        enabled: source.enabled,
        counts_toward_quorum: source.counts_toward_quorum,
        canonical_base_asset: source.canonical_base_asset.clone(),
        canonical_quote_asset: source.canonical_quote_asset.clone(),
    }
}

fn observe_case(fixture: &RealizedVolFixture) -> ObserveCase {
    let source = fixture
        .sources
        .first()
        .expect("realized-volatility fixture must contain a source");
    let observations = fixture
        .observations
        .iter()
        .map(|observation| RealizedVolObservation {
            source_id: source.source_id.clone(),
            source_class: source_class(&source.source_class),
            sample_kind: sample_kind(&source.sample_kind),
            price: observation.price,
            event_ts_ms: observation.event_ts_ms,
            recv_ts_ms: observation.recv_ts_ms,
        })
        .collect();
    ObserveCase {
        engine: RealizedVolEngine::from_config(engine_config(fixture))
            .expect("realized-volatility fixture must produce a valid engine"),
        observations,
        snapshot_as_of_ms: fixture.snapshot_as_of_ms,
    }
}

fn source_class(value: &str) -> RealizedVolSourceClass {
    match value {
        "spot_quote" => RealizedVolSourceClass::SpotQuote,
        value => panic!("unsupported benchmark source class: {value}"),
    }
}

fn sample_kind(value: &str) -> RealizedVolSampleKind {
    match value {
        "midpoint" => RealizedVolSampleKind::Midpoint,
        value => panic!("unsupported benchmark sample kind: {value}"),
    }
}
