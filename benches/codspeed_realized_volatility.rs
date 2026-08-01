mod support;

use std::{collections::BTreeSet, sync::OnceLock};

use bolt_v2::{
    bolt_v3_config::{RealizedVolatilitySurfaceBlock, realized_volatility_engine_config},
    bolt_v3_realized_volatility::{
        RealizedVolEngine, RealizedVolEngineConfig, RealizedVolJumpPolicy, RealizedVolNoiseMethod,
        RealizedVolObservation, RealizedVolPricingComponent, RealizedVolSourceStatus,
    },
};
use serde::Deserialize;

#[derive(Deserialize)]
struct FixtureFile {
    realized_volatility: RealizedVolFixture,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RealizedVolFixture {
    surface_id: String,
    snapshot_as_of_ms: u64,
    surface: RealizedVolatilitySurfaceBlock,
    observations: Vec<ObservationFixture>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ObservationFixture {
    source_id: String,
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
    FIXTURE.get_or_init(|| {
        let fixture = support::decode_fixtures::<FixtureFile>().realized_volatility;
        validate_fixture(&fixture);
        fixture
    })
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
    realized_volatility_engine_config(&fixture.surface_id, &fixture.surface)
        .expect("realized-volatility fixture must map through the production config converter")
}

fn observations(
    fixture: &RealizedVolFixture,
    config: &RealizedVolEngineConfig,
) -> Vec<RealizedVolObservation> {
    fixture
        .observations
        .iter()
        .map(|observation| {
            let source = config
                .sources
                .iter()
                .find(|source| source.source_id == observation.source_id)
                .expect("every observation must reference a configured source");
            RealizedVolObservation {
                source_id: source.source_id.clone(),
                source_class: source.source_class,
                sample_kind: source.sample_kind,
                price: observation.price,
                event_ts_ms: observation.event_ts_ms,
                recv_ts_ms: observation.recv_ts_ms,
            }
        })
        .collect()
}

fn observe_case(fixture: &RealizedVolFixture) -> ObserveCase {
    let config = engine_config(fixture);
    let observations = observations(fixture, &config);
    ObserveCase {
        engine: RealizedVolEngine::from_config(config)
            .expect("realized-volatility fixture must produce a valid engine"),
        observations,
        snapshot_as_of_ms: fixture.snapshot_as_of_ms,
    }
}

fn validate_fixture(fixture: &RealizedVolFixture) {
    let config = engine_config(fixture);
    assert_eq!(
        config.sources.len(),
        3,
        "production-shaped benchmark must exercise three quorum sources"
    );
    match &config.estimator.noise.method {
        RealizedVolNoiseMethod::Subsampled {
            subsamples,
            min_ready_subsamples,
        } => assert_eq!(
            (*subsamples, *min_ready_subsamples),
            (2, 2),
            "production-shaped benchmark must exercise both subsample lanes"
        ),
        method => panic!("benchmark must use subsampled noise estimation, got {method:?}"),
    }
    assert_eq!(
        config.estimator.jump.policy,
        RealizedVolJumpPolicy::Separate,
        "benchmark must exercise jump separation"
    );
    assert_eq!(
        config.estimator.pricing_component,
        RealizedVolPricingComponent::NoiseRobust,
        "benchmark must price from the noise-robust component"
    );

    let observations = observations(fixture, &config);
    let mut engine = RealizedVolEngine::from_config(config.clone())
        .expect("realized-volatility fixture must produce a valid scratch engine");
    for observation in observations {
        assert!(
            engine.observe(observation),
            "every benchmark observation must be accepted during untimed preflight"
        );
    }

    let snapshot = engine.snapshot_at(fixture.snapshot_as_of_ms);
    assert!(
        snapshot.ready,
        "benchmark snapshot must be ready, got blockers {:?}",
        snapshot.blocked_reasons
    );
    assert!(
        snapshot.blocked_reasons.is_empty(),
        "ready benchmark snapshot must not retain blockers"
    );
    assert!(
        snapshot.unknown_source_rejections.is_empty(),
        "benchmark observations must not include unknown sources"
    );
    assert_eq!(
        snapshot.pricing_component,
        RealizedVolPricingComponent::NoiseRobust
    );

    let expected_sources = config
        .sources
        .iter()
        .map(|source| source.source_id.as_str())
        .collect::<BTreeSet<_>>();
    let used_sources = snapshot
        .sources_used
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    assert_eq!(
        used_sources, expected_sources,
        "all configured sources must contribute to the aggregate"
    );
    assert_eq!(
        snapshot.source_diagnostics.len(),
        config.sources.len(),
        "every configured source must have a diagnostic"
    );
    for diagnostic in &snapshot.source_diagnostics {
        assert_eq!(
            diagnostic.status,
            RealizedVolSourceStatus::Ready,
            "source {} must reach the production estimator path",
            diagnostic.source_id
        );
        assert!(diagnostic.block_reason.is_none());
        assert!(diagnostic.last_rejected_reason.is_none());
        assert!(diagnostic.rejection_counters.is_empty());
        assert_component(
            diagnostic.measured_annualized_realized_vol_decimal,
            "source measured",
        );
        assert_component(
            diagnostic.noise_robust_annualized_realized_vol_decimal,
            "source noise-robust",
        );
        assert_component(
            diagnostic.continuous_annualized_realized_vol_decimal,
            "source continuous",
        );
        assert!(
            assert_component(
                diagnostic.jump_annualized_realized_vol_decimal,
                "source jump",
            ) > 0.0,
            "source {} must contain a separated jump",
            diagnostic.source_id
        );
    }

    let measured = assert_component(
        snapshot.measured_annualized_realized_vol_decimal,
        "aggregate measured",
    );
    let noise_robust = assert_component(
        snapshot.noise_robust_annualized_realized_vol_decimal,
        "aggregate noise-robust",
    );
    let continuous = assert_component(
        snapshot.continuous_annualized_realized_vol_decimal,
        "aggregate continuous",
    );
    let jump = assert_component(
        snapshot.jump_annualized_realized_vol_decimal,
        "aggregate jump",
    );
    assert!(jump > 0.0, "aggregate jump component must be positive");
    assert!(
        (measured - continuous).abs() > f64::EPSILON,
        "jump separation must distinguish measured and continuous volatility"
    );
    assert!(
        (measured - noise_robust).abs() > f64::EPSILON,
        "subsampling must distinguish measured and noise-robust volatility"
    );
}

fn assert_component(value: Option<f64>, label: &str) -> f64 {
    let value = value.unwrap_or_else(|| panic!("{label} component must be populated"));
    assert!(value.is_finite(), "{label} component must be finite");
    value
}
