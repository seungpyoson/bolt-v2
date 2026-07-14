use bolt_v2::bolt_v3_realized_volatility::{
    RealizedVolAggregation, RealizedVolBlockReason, RealizedVolCoarserGridPolicy,
    RealizedVolEngine, RealizedVolEngineConfig, RealizedVolJumpConfig, RealizedVolJumpPolicy,
    RealizedVolNoiseConfig, RealizedVolNoiseMethod, RealizedVolObservation,
    RealizedVolPricingComponent, RealizedVolSampleKind, RealizedVolSnapshot,
    RealizedVolSourceClass, RealizedVolSourceConfig, RealizedVolSourceRejectReason,
    RealizedVolSourceStatus,
};

const SURFACE_ID: &str = "<surface_id>";
const SOURCE_A: &str = "<SOURCE_ID_A>";
const SOURCE_B: &str = "<SOURCE_ID_B>";
const SOURCE_C: &str = "<SOURCE_ID_C>";
const SOURCE_D: &str = "<SOURCE_ID_D>";

fn source(source_id: &str) -> RealizedVolSourceConfig {
    RealizedVolSourceConfig {
        source_id: source_id.to_string(),
        data_client_id: "<DATA_CLIENT_ID>".to_string(),
        instrument_id: "<INSTRUMENT_ID>.<DATA_CLIENT_ID>".to_string(),
        source_class: RealizedVolSourceClass::SpotQuote,
        sample_kind: RealizedVolSampleKind::Midpoint,
        enabled: true,
        counts_toward_quorum: true,
        canonical_base_asset: "<BASE_ASSET>".to_string(),
        canonical_quote_asset: "<QUOTE_ASSET>".to_string(),
    }
}

fn config(source_ids: &[&str]) -> RealizedVolEngineConfig {
    RealizedVolEngineConfig {
        surface_id: SURFACE_ID.to_string(),
        window_ms: 4_000,
        sampling_interval_ms: 1_000,
        min_ready_sources: source_ids.len(),
        max_source_age_ms: 500,
        max_inter_sample_gap_ms: 2_000,
        min_coverage_ratio: 0.75,
        max_cross_source_dispersion: 0.50,
        seconds_per_annum: 31_536_000.0,
        aggregation: RealizedVolAggregation::UpperQuantile { quantile: 1.0 },
        estimator: bolt_v2::bolt_v3_realized_volatility::RealizedVolEstimatorConfig::measured(),
        sources: source_ids
            .iter()
            .map(|source_id| source(source_id))
            .collect(),
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

fn observation_with_receive(
    source_id: &str,
    price: f64,
    event_ts_ms: u64,
    recv_ts_ms: u64,
) -> RealizedVolObservation {
    RealizedVolObservation {
        recv_ts_ms,
        ..observation(source_id, price, event_ts_ms)
    }
}

fn observe_path(engine: &mut RealizedVolEngine, source_id: &str, prices: &[f64]) {
    for (index, price) in prices.iter().enumerate() {
        let ts_ms = (index as u64 + 1) * 1_000;
        assert!(engine.observe(observation(source_id, *price, ts_ms)));
    }
}

fn ready_rv_for_prices(prices: &[f64]) -> f64 {
    let mut engine = RealizedVolEngine::from_config(config(&[SOURCE_A])).unwrap();
    observe_path(&mut engine, SOURCE_A, prices);
    engine
        .snapshot_at(4_000)
        .annualized_realized_vol_decimal
        .expect("price path should publish ready RV")
}

fn assert_close(actual: f64, expected: f64, tolerance: f64) {
    assert!(
        (actual - expected).abs() <= tolerance,
        "expected {expected}, got {actual}"
    );
}

#[test]
fn rv_clock_domain_amendment_cutoff_excludes_accepted_observation_not_used_by_snapshot() {
    let mut engine = RealizedVolEngine::from_config(config(&[SOURCE_A])).unwrap();
    for (event_ts_ms, recv_ts_ms, price) in [
        (1_000, 1_100, 100.0),
        (2_000, 2_100, 101.0),
        (3_000, 3_100, 102.0),
        (4_000, 4_100, 103.0),
        (5_000, 50_000, 104.0),
    ] {
        assert!(engine.observe(observation_with_receive(
            SOURCE_A,
            price,
            event_ts_ms,
            recv_ts_ms,
        )));
    }

    let snapshot = engine.snapshot_at(4_000);

    assert!(snapshot.ready);
    assert_eq!(
        snapshot
            .latest_accepted_receive_ms
            .map(|stamp| stamp.value()),
        Some(4_100),
        "an accepted observation beyond the snapshot cutoff is retained but not causal"
    );
}

#[test]
fn rv_clock_domain_amendment_watermark_tracks_max_used_receive_across_every_grid() {
    let mut configurations = Vec::new();
    configurations.push(config(&[SOURCE_A]));

    let mut coarse = config(&[SOURCE_A]);
    coarse.max_source_age_ms = 1_000;
    coarse.estimator.noise = RealizedVolNoiseConfig {
        method: RealizedVolNoiseMethod::CoarserGrid {
            coarse_sampling_interval_ms: 2_000,
            policy: RealizedVolCoarserGridPolicy::CoarseOnly,
        },
    };
    configurations.push(coarse);

    let mut subsampled = config(&[SOURCE_A]);
    subsampled.max_source_age_ms = 1_000;
    subsampled.estimator.noise = RealizedVolNoiseConfig {
        method: RealizedVolNoiseMethod::Subsampled {
            subsamples: 2,
            min_ready_subsamples: 2,
        },
    };
    configurations.push(subsampled);

    for cfg in configurations {
        let mut engine = RealizedVolEngine::from_config(cfg).unwrap();
        for (event_ts_ms, recv_ts_ms, price) in [
            (1_000, 10_000, 100.0),
            (2_000, 2_100, 101.0),
            (3_000, 3_100, 102.0),
            (4_000, 4_100, 103.0),
        ] {
            assert!(engine.observe(observation_with_receive(
                SOURCE_A,
                price,
                event_ts_ms,
                recv_ts_ms,
            )));
        }

        let snapshot = engine.snapshot_at(4_000);
        assert!(snapshot.ready);
        assert_eq!(
            snapshot
                .latest_accepted_receive_ms
                .map(|stamp| stamp.value()),
            Some(10_000),
            "base, coarse, and subsampled estimators must retain the identity of every used sample"
        );
    }
}

#[test]
fn rv_clock_domain_amendment_subsampled_lane_adds_off_base_grid_sample_to_watermark() {
    let mut cfg = config(&[SOURCE_A]);
    cfg.max_source_age_ms = 1_000;
    cfg.estimator.noise = RealizedVolNoiseConfig {
        method: RealizedVolNoiseMethod::Subsampled {
            subsamples: 2,
            min_ready_subsamples: 2,
        },
    };
    let mut base_engine = RealizedVolEngine::from_config(config(&[SOURCE_A])).unwrap();
    let mut engine = RealizedVolEngine::from_config(cfg).unwrap();
    for (event_ts_ms, recv_ts_ms, price) in [
        (1_000, 1_100, 100.0),
        (1_400, 20_000, 110.0),
        (1_900, 1_900, 101.0),
        (2_400, 2_400, 102.0),
        (2_900, 2_900, 103.0),
        (3_400, 3_400, 104.0),
        (3_900, 3_900, 105.0),
    ] {
        let observation = observation_with_receive(SOURCE_A, price, event_ts_ms, recv_ts_ms);
        assert!(base_engine.observe(observation.clone()));
        assert!(engine.observe(observation));
    }

    let base_snapshot = base_engine.snapshot_at(4_000);
    let snapshot = engine.snapshot_at(4_000);

    assert!(base_snapshot.ready);
    assert_eq!(
        base_snapshot
            .latest_accepted_receive_ms
            .map(|stamp| stamp.value()),
        Some(3_900),
        "the base grid must supersede the off-grid observation before its next sample point"
    );
    assert!(snapshot.ready);
    assert_eq!(snapshot.source_diagnostics[0].grid_sample_count, 4);
    assert_eq!(
        snapshot
            .latest_accepted_receive_ms
            .map(|stamp| stamp.value()),
        Some(20_000),
        "the successful offset lane must contribute its off-base-grid sample identity"
    );
}

#[test]
fn rv_clock_domain_amendment_trimmed_source_remains_a_causal_contributor() {
    let mut cfg = config(&[SOURCE_A, SOURCE_B, SOURCE_C, SOURCE_D]);
    cfg.aggregation = RealizedVolAggregation::TrimmedMean {
        trim_fraction: 0.25,
    };
    cfg.max_cross_source_dispersion = 10_000.0;
    let mut engine = RealizedVolEngine::from_config(cfg).unwrap();
    for (source_id, receive_offset, prices) in [
        (SOURCE_A, 20_000, [100.0, 100.0, 100.0, 100.0]),
        (SOURCE_B, 0, [100.0, 101.0, 102.0, 103.0]),
        (SOURCE_C, 0, [100.0, 102.0, 104.0, 106.0]),
        (SOURCE_D, 0, [100.0, 125.0, 75.0, 150.0]),
    ] {
        for (index, price) in prices.into_iter().enumerate() {
            let event_ts_ms = (index as u64 + 1) * 1_000;
            assert!(engine.observe(observation_with_receive(
                source_id,
                price,
                event_ts_ms,
                event_ts_ms + receive_offset,
            )));
        }
    }

    let snapshot = engine.snapshot_at(4_000);

    assert!(snapshot.ready);
    assert_eq!(
        snapshot
            .latest_accepted_receive_ms
            .map(|stamp| stamp.value()),
        Some(24_000),
        "a numerically trimmed ready source still affects quorum, dispersion, and selection"
    );
}

#[test]
fn rv_clock_domain_amendment_quantile_unselected_source_remains_a_causal_contributor() {
    let mut cfg = config(&[SOURCE_A, SOURCE_B, SOURCE_C]);
    cfg.aggregation = RealizedVolAggregation::UpperQuantile { quantile: 1.0 };
    cfg.max_cross_source_dispersion = 10_000.0;
    let mut engine = RealizedVolEngine::from_config(cfg).unwrap();
    for (source_id, receive_offset, prices) in [
        (SOURCE_A, 20_000, [100.0, 100.1, 100.2, 100.3]),
        (SOURCE_B, 0, [100.0, 101.0, 102.0, 103.0]),
        (SOURCE_C, 0, [100.0, 125.0, 75.0, 150.0]),
    ] {
        for (index, price) in prices.into_iter().enumerate() {
            let event_ts_ms = (index as u64 + 1) * 1_000;
            assert!(engine.observe(observation_with_receive(
                source_id,
                price,
                event_ts_ms,
                event_ts_ms + receive_offset,
            )));
        }
    }

    let snapshot = engine.snapshot_at(4_000);

    assert!(snapshot.ready);
    assert_eq!(
        snapshot
            .latest_accepted_receive_ms
            .map(|stamp| stamp.value()),
        Some(24_000),
        "a quantile-unselected ready source remains causal through quorum and dispersion"
    );
}

#[test]
fn rv_clock_domain_amendment_rejected_observation_never_enters_causal_set() {
    let mut engine = RealizedVolEngine::from_config(config(&[SOURCE_A])).unwrap();
    for (event_ts_ms, recv_ts_ms, price) in [
        (1_000, 1_100, 100.0),
        (2_000, 2_100, 101.0),
        (3_000, 3_100, 102.0),
        (4_000, 4_100, 103.0),
    ] {
        assert!(engine.observe(observation_with_receive(
            SOURCE_A,
            price,
            event_ts_ms,
            recv_ts_ms,
        )));
    }
    assert!(!engine.observe(observation_with_receive(SOURCE_A, 999.0, 3_500, 50_000,)));

    let snapshot = engine.snapshot_at(4_000);

    assert!(snapshot.ready);
    assert_eq!(
        snapshot
            .latest_accepted_receive_ms
            .map(|stamp| stamp.value()),
        Some(4_100),
        "rejected observations may update diagnostics but never the causal watermark"
    );
}

#[test]
fn source_id_quorum_snapshot_records_audit_fields() {
    let mut engine = RealizedVolEngine::from_config(config(&[SOURCE_A, SOURCE_B])).unwrap();
    observe_path(&mut engine, SOURCE_A, &[100.0, 101.0, 102.0, 103.0]);
    observe_path(&mut engine, SOURCE_B, &[200.0, 202.0, 204.0, 206.0]);

    let snapshot = engine.snapshot_at(4_000);

    assert!(snapshot.ready);
    assert_eq!(snapshot.surface_id, SURFACE_ID);
    assert_eq!(
        snapshot.sources_used,
        vec![SOURCE_A.to_string(), SOURCE_B.to_string()]
    );
    assert_eq!(
        snapshot.aggregate_method,
        RealizedVolAggregation::UpperQuantile { quantile: 1.0 }
    );
    assert_eq!(snapshot.seconds_per_annum, 31_536_000.0);
    assert!(snapshot.annualized_realized_vol_decimal.unwrap() > 0.0);
    assert!(snapshot.blocked_reasons.is_empty());
    assert_eq!(snapshot.source_diagnostics.len(), 2);
    assert!(
        snapshot
            .source_diagnostics
            .iter()
            .all(|d| d.status == RealizedVolSourceStatus::Ready)
    );
}

#[test]
fn cross_source_dispersion_blocks_instead_of_publishing_low_rv() {
    let mut engine = RealizedVolEngine::from_config(config(&[SOURCE_A, SOURCE_B])).unwrap();
    observe_path(&mut engine, SOURCE_A, &[100.0, 100.1, 100.2, 100.3]);
    observe_path(&mut engine, SOURCE_B, &[100.0, 105.0, 95.0, 110.0]);

    let snapshot = engine.snapshot_at(4_000);

    assert!(!snapshot.ready);
    assert_eq!(snapshot.annualized_realized_vol_decimal, None);
    assert!(
        snapshot
            .blocked_reasons
            .contains(&RealizedVolBlockReason::CrossSourceDispersion)
    );
}

#[test]
fn median_aggregation_ignores_one_extreme_ready_source_when_quorum_satisfied() {
    let mut cfg = config(&[SOURCE_A, SOURCE_B, SOURCE_C]);
    cfg.aggregation = RealizedVolAggregation::Median;
    cfg.max_cross_source_dispersion = 10_000.0;
    let expected_mid = ready_rv_for_prices(&[100.0, 101.0, 102.0, 103.0]);
    let mut engine = RealizedVolEngine::from_config(cfg).unwrap();
    observe_path(&mut engine, SOURCE_A, &[100.0, 100.1, 100.2, 100.3]);
    observe_path(&mut engine, SOURCE_B, &[100.0, 101.0, 102.0, 103.0]);
    observe_path(&mut engine, SOURCE_C, &[100.0, 125.0, 75.0, 150.0]);

    let snapshot = engine.snapshot_at(4_000);

    assert!(snapshot.ready);
    let actual = snapshot.annualized_realized_vol_decimal.unwrap();
    assert!(
        (actual - expected_mid).abs() < 1e-9,
        "median aggregation should use middle contributor; expected {expected_mid}, got {actual}"
    );
}

#[test]
fn trimmed_mean_aggregation_trims_extreme_ready_sources() {
    let mut cfg = config(&[SOURCE_A, SOURCE_B, SOURCE_C, SOURCE_D]);
    cfg.aggregation = RealizedVolAggregation::TrimmedMean {
        trim_fraction: 0.25,
    };
    cfg.max_cross_source_dispersion = 10_000.0;
    let expected_low_mid = ready_rv_for_prices(&[100.0, 101.0, 102.0, 103.0]);
    let expected_high_mid = ready_rv_for_prices(&[100.0, 102.0, 104.0, 106.0]);
    let expected = (expected_low_mid + expected_high_mid) / 2.0;
    let mut engine = RealizedVolEngine::from_config(cfg).unwrap();
    observe_path(&mut engine, SOURCE_A, &[100.0, 100.0, 100.0, 100.0]);
    observe_path(&mut engine, SOURCE_B, &[100.0, 101.0, 102.0, 103.0]);
    observe_path(&mut engine, SOURCE_C, &[100.0, 102.0, 104.0, 106.0]);
    observe_path(&mut engine, SOURCE_D, &[100.0, 125.0, 75.0, 150.0]);

    let snapshot = engine.snapshot_at(4_000);

    assert!(snapshot.ready);
    assert_close(
        snapshot.annualized_realized_vol_decimal.unwrap(),
        expected,
        1e-9,
    );
}

#[test]
fn median_with_upper_quantile_guard_blends_median_and_guard_value() {
    let mut cfg = config(&[SOURCE_A, SOURCE_B, SOURCE_C]);
    cfg.aggregation = RealizedVolAggregation::MedianWithUpperQuantileGuard {
        upper_quantile: 1.0,
        guard_weight: 0.25,
    };
    cfg.max_cross_source_dispersion = 10_000.0;
    let expected_median = ready_rv_for_prices(&[100.0, 101.0, 102.0, 103.0]);
    let expected_guard = ready_rv_for_prices(&[100.0, 125.0, 75.0, 150.0]);
    let expected = expected_median.mul_add(0.75, expected_guard * 0.25);
    let mut engine = RealizedVolEngine::from_config(cfg).unwrap();
    observe_path(&mut engine, SOURCE_A, &[100.0, 100.1, 100.2, 100.3]);
    observe_path(&mut engine, SOURCE_B, &[100.0, 101.0, 102.0, 103.0]);
    observe_path(&mut engine, SOURCE_C, &[100.0, 125.0, 75.0, 150.0]);

    let snapshot = engine.snapshot_at(4_000);

    assert!(snapshot.ready);
    assert_close(
        snapshot.annualized_realized_vol_decimal.unwrap(),
        expected,
        1e-9,
    );
}

#[test]
fn jump_separation_publishes_continuous_and_jump_components() {
    let mut cfg = config(&[SOURCE_A]);
    cfg.estimator.jump = RealizedVolJumpConfig {
        policy: RealizedVolJumpPolicy::Separate,
    };
    let mut engine = RealizedVolEngine::from_config(cfg).unwrap();
    observe_path(&mut engine, SOURCE_A, &[100.0, 100.2, 140.0, 140.2]);

    let snapshot = engine.snapshot_at(4_000);

    assert!(snapshot.ready);
    assert!(snapshot.measured_annualized_realized_vol_decimal.unwrap() > 0.0);
    assert!(
        snapshot.jump_annualized_realized_vol_decimal.unwrap() > 0.0,
        "large isolated return should be reported as jump component"
    );
    assert!(
        snapshot.continuous_annualized_realized_vol_decimal.unwrap()
            < snapshot.measured_annualized_realized_vol_decimal.unwrap(),
        "continuous component should not equal measured RV when a jump is separated"
    );
}

#[test]
fn subsampled_rv_reduces_alternating_midpoint_bounce_vs_base_grid() {
    let mut base_cfg = config(&[SOURCE_A]);
    base_cfg.max_source_age_ms = 1_000;
    let mut robust_cfg = base_cfg.clone();
    robust_cfg.estimator.noise = RealizedVolNoiseConfig {
        method: RealizedVolNoiseMethod::Subsampled {
            subsamples: 2,
            min_ready_subsamples: 2,
        },
    };
    let mut base = RealizedVolEngine::from_config(base_cfg).unwrap();
    let mut robust = RealizedVolEngine::from_config(robust_cfg).unwrap();
    for (ts_ms, price) in [
        (1_000, 100.0),
        (2_000, 101.0),
        (3_000, 100.0),
        (4_000, 101.0),
    ] {
        let observation = observation(SOURCE_A, price, ts_ms);
        assert!(base.observe(observation.clone()));
        assert!(robust.observe(observation));
    }

    let base_snapshot = base.snapshot_at(4_000);
    let robust_snapshot = robust.snapshot_at(4_000);

    assert!(base_snapshot.ready);
    assert!(robust_snapshot.ready);
    assert!(
        robust_snapshot
            .noise_robust_annualized_realized_vol_decimal
            .unwrap()
            < base_snapshot
                .measured_annualized_realized_vol_decimal
                .unwrap(),
        "subsampled RV should reduce deterministic bid/ask bounce"
    );
}

#[test]
fn subsampled_rv_uses_deterministic_offset_grids_not_raw_tick_thinning() {
    let mut robust_cfg = config(&[SOURCE_A]);
    robust_cfg.max_source_age_ms = 1_000;
    robust_cfg.estimator.noise = RealizedVolNoiseConfig {
        method: RealizedVolNoiseMethod::Subsampled {
            subsamples: 2,
            min_ready_subsamples: 2,
        },
    };
    let mut robust = RealizedVolEngine::from_config(robust_cfg).unwrap();
    for (ts_ms, price) in [
        (1_000, 100.0),
        (2_000, 101.0),
        (3_000, 100.0),
        (4_000, 101.0),
    ] {
        assert!(robust.observe(observation(SOURCE_A, price, ts_ms)));
    }

    let snapshot = robust.snapshot_at(4_000);

    assert!(snapshot.ready);
    let annualization = 31_536_000.0;
    let base_sum = (101.0_f64 / 100.0).ln().powi(2)
        + (100.0_f64 / 101.0).ln().powi(2)
        + (101.0_f64 / 100.0).ln().powi(2);
    let offset_sum = (101.0_f64 / 100.0).ln().powi(2) + (100.0_f64 / 101.0).ln().powi(2);
    let base_variance = (base_sum / 3.0) * annualization;
    let offset_variance = (offset_sum / 2.0) * annualization;
    let expected_noise_robust_rv = ((base_variance + offset_variance) / 2.0).sqrt();
    let actual = snapshot
        .noise_robust_annualized_realized_vol_decimal
        .expect("subsampled RV should be published");

    assert_close(actual, expected_noise_robust_rv, 1e-9);
    assert!(
        actual > 0.0,
        "offset-grid subsampling should not collapse this path to zero"
    );
}

#[test]
fn coarser_grid_policy_selects_coarse_only_component() {
    let mut cfg = config(&[SOURCE_A]);
    cfg.estimator.noise = RealizedVolNoiseConfig {
        method: RealizedVolNoiseMethod::CoarserGrid {
            coarse_sampling_interval_ms: 2_000,
            policy: RealizedVolCoarserGridPolicy::CoarseOnly,
        },
    };
    cfg.estimator.pricing_component = RealizedVolPricingComponent::NoiseRobust;
    let mut engine = RealizedVolEngine::from_config(cfg).unwrap();
    observe_path(&mut engine, SOURCE_A, &[100.0, 100.0, 110.0, 110.0]);

    let snapshot = engine.snapshot_at(4_000);

    assert!(snapshot.ready);
    let expected_coarse = ((110.0_f64 / 100.0).ln().powi(2) / 2.0 * 31_536_000.0).sqrt();
    let noise_robust = snapshot
        .noise_robust_annualized_realized_vol_decimal
        .expect("coarse-only RV should be present");
    assert_close(noise_robust, expected_coarse, 1e-9);
    assert_close(
        snapshot.annualized_realized_vol_decimal.unwrap(),
        expected_coarse,
        1e-9,
    );
}

#[test]
fn coarser_grid_min_base_coarse_uses_lower_component() {
    let mut cfg = config(&[SOURCE_A]);
    cfg.estimator.noise = RealizedVolNoiseConfig {
        method: RealizedVolNoiseMethod::CoarserGrid {
            coarse_sampling_interval_ms: 2_000,
            policy: RealizedVolCoarserGridPolicy::MinBaseCoarse,
        },
    };
    cfg.estimator.pricing_component = RealizedVolPricingComponent::NoiseRobust;
    let mut engine = RealizedVolEngine::from_config(cfg).unwrap();
    observe_path(&mut engine, SOURCE_A, &[100.0, 100.0, 110.0, 110.0]);

    let snapshot = engine.snapshot_at(4_000);

    assert!(snapshot.ready);
    let expected_base = ((110.0_f64 / 100.0).ln().powi(2) / 3.0 * 31_536_000.0).sqrt();
    let noise_robust = snapshot
        .noise_robust_annualized_realized_vol_decimal
        .expect("min-base-coarse RV should be present");
    assert_close(noise_robust, expected_base, 1e-9);
    assert_close(
        snapshot.annualized_realized_vol_decimal.unwrap(),
        expected_base,
        1e-9,
    );
}

#[test]
fn jump_separation_preserves_measured_variance_identity_when_noise_robust_prices() {
    let mut cfg = config(&[SOURCE_A]);
    cfg.estimator.noise = RealizedVolNoiseConfig {
        method: RealizedVolNoiseMethod::Subsampled {
            subsamples: 2,
            min_ready_subsamples: 2,
        },
    };
    cfg.estimator.jump = RealizedVolJumpConfig {
        policy: RealizedVolJumpPolicy::Separate,
    };
    cfg.estimator.pricing_component = RealizedVolPricingComponent::NoiseRobust;
    let mut engine = RealizedVolEngine::from_config(cfg).unwrap();
    observe_path(&mut engine, SOURCE_A, &[100.0, 100.2, 140.0, 140.2]);

    let snapshot = engine.snapshot_at(4_000);

    assert!(snapshot.ready);
    let measured = snapshot
        .measured_annualized_realized_vol_decimal
        .expect("measured RV should be present");
    let continuous = snapshot
        .continuous_annualized_realized_vol_decimal
        .expect("continuous RV should be present");
    let jump = snapshot
        .jump_annualized_realized_vol_decimal
        .expect("jump RV should be present");
    let noise_robust = snapshot
        .noise_robust_annualized_realized_vol_decimal
        .expect("noise robust RV should be present");
    let priced = snapshot
        .annualized_realized_vol_decimal
        .expect("final priced RV should be present");

    assert_close(measured.powi(2), continuous.powi(2) + jump.powi(2), 1e-9);
    assert_close(priced, noise_robust, 1e-12);
    assert!(jump > 0.0, "large return should remain visible as jump");
}

#[test]
fn fixed_grid_coverage_is_required_before_source_is_ready() {
    let mut engine = RealizedVolEngine::from_config(config(&[SOURCE_A])).unwrap();
    assert!(engine.observe(observation(SOURCE_A, 100.0, 1_000)));
    assert!(engine.observe(observation(SOURCE_A, 104.0, 4_000)));

    let snapshot = engine.snapshot_at(4_000);

    assert!(!snapshot.ready);
    assert_eq!(
        snapshot.blocked_reasons,
        vec![RealizedVolBlockReason::QuorumNotReady]
    );
    assert_eq!(
        snapshot.source_diagnostics[0].status,
        RealizedVolSourceStatus::Blocked
    );
    assert_eq!(
        snapshot.source_diagnostics[0].block_reason,
        Some(RealizedVolBlockReason::CoverageBelowMinimum)
    );
    assert!(snapshot.source_diagnostics[0].coverage_ratio < 0.75);
}

#[test]
fn source_level_readiness_failures_do_not_block_satisfied_quorum() {
    let mut cfg = config(&[SOURCE_A, SOURCE_B]);
    cfg.min_ready_sources = 1;
    let mut engine = RealizedVolEngine::from_config(cfg).unwrap();
    observe_path(&mut engine, SOURCE_A, &[100.0, 101.0, 102.0, 103.0]);
    assert!(engine.observe(observation(SOURCE_B, 100.0, 1_000)));
    assert!(engine.observe(observation(SOURCE_B, 104.0, 4_000)));

    let snapshot = engine.snapshot_at(4_000);

    assert!(snapshot.ready);
    assert!(snapshot.blocked_reasons.is_empty());
    assert_eq!(snapshot.sources_used, vec![SOURCE_A.to_string()]);
    let blocked_diagnostic = snapshot
        .source_diagnostics
        .iter()
        .find(|diagnostic| diagnostic.source_id == SOURCE_B)
        .expect("non-ready source should remain diagnostic-visible");
    assert_eq!(blocked_diagnostic.status, RealizedVolSourceStatus::Blocked);
    assert_eq!(
        blocked_diagnostic.block_reason,
        Some(RealizedVolBlockReason::CoverageBelowMinimum)
    );
}

#[test]
fn same_event_update_requires_strictly_larger_receive_time() {
    let mut engine = RealizedVolEngine::from_config(config(&[SOURCE_A])).unwrap();
    assert!(engine.observe(RealizedVolObservation {
        recv_ts_ms: 1_200,
        ..observation(SOURCE_A, 100.0, 1_000)
    }));

    assert!(!engine.observe(RealizedVolObservation {
        recv_ts_ms: 1_100,
        price: 101.0,
        ..observation(SOURCE_A, 101.0, 1_000)
    }));

    let snapshot = engine.snapshot_at(1_000);
    let diagnostic = &snapshot.source_diagnostics[0];
    assert_eq!(
        diagnostic.last_rejected_reason,
        Some(RealizedVolSourceRejectReason::StaleSameEventUpdate)
    );
}

#[test]
fn recovered_source_reports_ready_without_historical_rejection_as_current_blocker() {
    let mut engine = RealizedVolEngine::from_config(config(&[SOURCE_A])).unwrap();
    assert!(!engine.observe(RealizedVolObservation {
        source_class: RealizedVolSourceClass::Trade,
        ..observation(SOURCE_A, 100.0, 500)
    }));
    observe_path(&mut engine, SOURCE_A, &[100.0, 101.0, 102.0, 103.0]);

    let snapshot = engine.snapshot_at(4_000);
    let diagnostic = &snapshot.source_diagnostics[0];

    assert!(snapshot.ready);
    assert_eq!(diagnostic.status, RealizedVolSourceStatus::Ready);
    assert_eq!(
        diagnostic.last_rejected_reason,
        Some(RealizedVolSourceRejectReason::SourceClassMismatch)
    );
    assert_eq!(
        diagnostic
            .rejection_counters
            .get(&RealizedVolSourceRejectReason::SourceClassMismatch)
            .copied(),
        Some(1)
    );
    assert_eq!(diagnostic.block_reason, None);
}

#[test]
fn unknown_source_rejections_are_audited_without_mutating_configured_sources() {
    let mut engine = RealizedVolEngine::from_config(config(&[SOURCE_A])).unwrap();
    assert!(!engine.observe(observation("<unknown_source_id>", 100.0, 1_000)));

    let snapshot = engine.snapshot_at(1_000);

    assert_eq!(
        snapshot
            .unknown_source_rejections
            .get("<unknown_source_id>")
            .copied(),
        Some(1)
    );
    assert_eq!(snapshot.source_diagnostics.len(), 1);
    assert_eq!(snapshot.source_diagnostics[0].source_id, SOURCE_A);
}

#[test]
fn disabled_configured_source_rejections_remain_auditable_without_quorum_participation() {
    let mut config = config(&[SOURCE_A, SOURCE_B]);
    config.min_ready_sources = 1;
    config.sources[1].enabled = false;
    let mut engine = RealizedVolEngine::from_config(config).unwrap();
    observe_path(&mut engine, SOURCE_A, &[100.0, 101.0, 102.0, 103.0]);
    assert!(!engine.observe(observation(SOURCE_B, 200.0, 1_000)));

    let snapshot = engine.snapshot_at(4_000);

    assert!(snapshot.ready);
    assert_eq!(snapshot.sources_used, vec![SOURCE_A.to_string()]);
    let disabled = snapshot
        .source_diagnostics
        .iter()
        .find(|diagnostic| diagnostic.source_id == SOURCE_B)
        .expect("disabled configured source should remain visible in diagnostics");
    assert_eq!(disabled.status, RealizedVolSourceStatus::DiagnosticOnly);
    assert_eq!(
        disabled.last_rejected_reason,
        Some(RealizedVolSourceRejectReason::DisabledSource)
    );
    assert_eq!(
        disabled
            .rejection_counters
            .get(&RealizedVolSourceRejectReason::DisabledSource)
            .copied(),
        Some(1)
    );
}

#[test]
fn disabled_source_diagnostic_exports_config_participation_without_observations() {
    let mut config = config(&[SOURCE_A, SOURCE_B]);
    config.min_ready_sources = 1;
    config.sources[1].enabled = false;
    config.sources[1].counts_toward_quorum = false;
    let mut engine = RealizedVolEngine::from_config(config).unwrap();
    observe_path(&mut engine, SOURCE_A, &[100.0, 101.0, 102.0, 103.0]);

    let snapshot = engine.snapshot_at(4_000);

    assert!(snapshot.ready);
    let disabled = snapshot
        .source_diagnostics
        .iter()
        .find(|diagnostic| diagnostic.source_id == SOURCE_B)
        .expect("disabled configured source should remain visible in diagnostics");
    assert!(!disabled.enabled);
    assert!(!disabled.counts_toward_quorum);
    assert_eq!(disabled.status, RealizedVolSourceStatus::DiagnosticOnly);
    assert_eq!(disabled.last_rejected_reason, None);
}

#[test]
fn enabled_non_quorum_source_with_live_observations_remains_diagnostic_only() {
    let mut config = config(&[SOURCE_A, SOURCE_B]);
    config.min_ready_sources = 1;
    config.sources[1].counts_toward_quorum = false;
    let mut engine = RealizedVolEngine::from_config(config).unwrap();
    observe_path(&mut engine, SOURCE_A, &[100.0, 101.0, 102.0, 103.0]);
    observe_path(&mut engine, SOURCE_B, &[200.0, 202.0, 204.0, 206.0]);

    let snapshot = engine.snapshot_at(4_000);

    assert!(snapshot.ready);
    assert_eq!(snapshot.sources_used, vec![SOURCE_A.to_string()]);
    let diagnostic_only = snapshot
        .source_diagnostics
        .iter()
        .find(|diagnostic| diagnostic.source_id == SOURCE_B)
        .expect("non-quorum source should remain visible in diagnostics");
    assert!(diagnostic_only.enabled);
    assert!(!diagnostic_only.counts_toward_quorum);
    assert_eq!(
        diagnostic_only.status,
        RealizedVolSourceStatus::DiagnosticOnly
    );
    assert!(
        diagnostic_only
            .annualized_realized_vol_decimal
            .is_some_and(|value| value > 0.0),
        "diagnostic-only source should still compute its own RV components"
    );
}

#[test]
fn observation_validation_uses_only_same_domain_timestamp_ordering() {
    let mut engine = RealizedVolEngine::from_config(config(&[SOURCE_A])).unwrap();
    assert!(engine.observe(observation(SOURCE_A, 100.0, 1_000)));

    let cases = [
        (
            RealizedVolObservation {
                event_ts_ms: 900,
                recv_ts_ms: 900,
                ..observation(SOURCE_A, 101.0, 900)
            },
            RealizedVolSourceRejectReason::EventTimeRegression,
        ),
        (
            observation(SOURCE_A, 100.0, 1_000),
            RealizedVolSourceRejectReason::DuplicateTimestamp,
        ),
    ];

    for (observation, reason) in cases {
        assert!(!engine.observe(observation));
        let snapshot = engine.snapshot_at(2_000);
        assert_eq!(
            snapshot.source_diagnostics[0].last_rejected_reason,
            Some(reason)
        );
    }

    assert!(engine.observe(RealizedVolObservation {
        event_ts_ms: 2_000,
        recv_ts_ms: 1_999,
        ..observation(SOURCE_A, 101.0, 2_000)
    }));
    assert!(engine.observe(RealizedVolObservation {
        event_ts_ms: 3_000,
        recv_ts_ms: 4_500,
        ..observation(SOURCE_A, 102.0, 3_000)
    }));
}

#[test]
fn flat_valid_source_publishes_zero_realized_volatility() {
    let mut engine = RealizedVolEngine::from_config(config(&[SOURCE_A])).unwrap();
    observe_path(&mut engine, SOURCE_A, &[100.0, 100.0, 100.0, 100.0]);

    let snapshot = engine.snapshot_at(4_000);

    assert!(snapshot.ready);
    assert_eq!(snapshot.annualized_realized_vol_decimal, Some(0.0));
    assert_eq!(snapshot.ready_realized_vol().map(|rv| rv.get()), Some(0.0));
}

#[test]
fn ready_realized_vol_accessor_requires_snapshot_readiness() {
    let mut snapshot = RealizedVolSnapshot::invalid_config(
        SURFACE_ID,
        4_000,
        RealizedVolAggregation::UpperQuantile { quantile: 1.0 },
        31_536_000.0,
        "<config_fingerprint>",
    );
    snapshot.annualized_realized_vol_decimal = Some(0.0);

    assert_eq!(snapshot.ready_realized_vol(), None);

    snapshot.ready = true;
    assert_eq!(snapshot.ready_realized_vol(), None);
}

#[test]
fn valid_realized_vol_rejects_negative_and_non_finite_values() {
    use bolt_v2::bolt_v3_realized_volatility::ValidRealizedVol;

    assert_eq!(ValidRealizedVol::new(0.0).map(|rv| rv.get()), Some(0.0));
    assert_eq!(ValidRealizedVol::new(1.0).map(|rv| rv.get()), Some(1.0));
    assert_eq!(ValidRealizedVol::new(-1.0), None);
    assert_eq!(ValidRealizedVol::new(f64::NAN), None);
    assert_eq!(ValidRealizedVol::new(f64::INFINITY), None);
    assert_eq!(ValidRealizedVol::new(f64::NEG_INFINITY), None);
}

#[test]
fn zero_aggregate_with_divergent_ready_sources_blocks_dispersion() {
    let mut cfg = config(&[SOURCE_A, SOURCE_B]);
    cfg.aggregation = RealizedVolAggregation::UpperQuantile { quantile: 0.5 };
    let mut engine = RealizedVolEngine::from_config(cfg).unwrap();
    observe_path(&mut engine, SOURCE_A, &[100.0, 100.0, 100.0, 100.0]);
    observe_path(&mut engine, SOURCE_B, &[100.0, 110.0, 90.0, 120.0]);

    let snapshot = engine.snapshot_at(4_000);

    assert!(!snapshot.ready);
    assert_eq!(snapshot.annualized_realized_vol_decimal, None);
    assert!(
        snapshot
            .blocked_reasons
            .contains(&RealizedVolBlockReason::CrossSourceDispersion)
    );
}

#[test]
fn fresh_pre_window_observation_can_seed_first_grid_cell() {
    let mut cfg = config(&[SOURCE_A]);
    cfg.max_source_age_ms = 1_500;
    let mut engine = RealizedVolEngine::from_config(cfg).unwrap();
    assert!(engine.observe(observation(SOURCE_A, 100.0, 750)));
    assert!(engine.observe(observation(SOURCE_A, 101.0, 3_000)));
    assert!(engine.observe(observation(SOURCE_A, 102.0, 4_000)));
    assert!(engine.observe(observation(SOURCE_A, 103.0, 5_000)));

    let snapshot = engine.snapshot_at(5_000);

    assert!(snapshot.ready);
    assert_eq!(snapshot.source_diagnostics[0].grid_sample_count, 4);
}

#[test]
fn snapshot_at_u64_max_does_not_loop_when_grid_increment_overflows() {
    let mut config = config(&[SOURCE_A]);
    config.window_ms = 1;
    config.sampling_interval_ms = 1;
    config.min_coverage_ratio = 0.0 + f64::EPSILON;
    let engine = RealizedVolEngine::from_config(config).unwrap();

    let snapshot = engine.snapshot_at(u64::MAX);

    assert_eq!(snapshot.as_of_ms, u64::MAX);
    assert!(!snapshot.ready);
}

#[test]
fn config_fingerprint_changes_when_policy_changes() {
    let baseline = RealizedVolEngine::from_config(config(&[SOURCE_A])).unwrap();
    let mut changed_config = config(&[SOURCE_A]);
    changed_config.max_source_age_ms += 1;
    let changed = RealizedVolEngine::from_config(changed_config).unwrap();

    assert_ne!(
        baseline.snapshot_at(4_000).config_fingerprint,
        changed.snapshot_at(4_000).config_fingerprint
    );
}

#[test]
fn config_fingerprint_is_stable_across_source_order() {
    let ordered = RealizedVolEngine::from_config(config(&[SOURCE_A, SOURCE_B])).unwrap();
    let mut reversed_config = config(&[SOURCE_A, SOURCE_B]);
    reversed_config.sources.reverse();
    let reversed = RealizedVolEngine::from_config(reversed_config).unwrap();

    assert_eq!(
        ordered.snapshot_at(4_000).config_fingerprint,
        reversed.snapshot_at(4_000).config_fingerprint
    );
}

#[test]
fn engine_config_validation_matches_root_policy_bounds() {
    let mut interval_exceeds_window = config(&[SOURCE_A]);
    interval_exceeds_window.window_ms = 1_000;
    interval_exceeds_window.sampling_interval_ms = 2_000;
    let error = RealizedVolEngine::from_config(interval_exceeds_window)
        .expect_err("engine constructor must reject sample intervals larger than the window");
    assert!(
        error.contains("window_ms"),
        "unexpected validation error: {error}"
    );

    let mut nan_coverage = config(&[SOURCE_A]);
    nan_coverage.min_coverage_ratio = f64::NAN;
    let error = RealizedVolEngine::from_config(nan_coverage)
        .expect_err("engine constructor must reject NaN coverage ratios");
    assert!(
        error.contains("min_coverage_ratio"),
        "unexpected validation error: {error}"
    );
}

#[test]
fn forecast_pricing_component_is_rejected_until_forecast_policy_is_enabled() {
    let mut cfg = config(&[SOURCE_A]);
    cfg.estimator.pricing_component = RealizedVolPricingComponent::Forecast;

    let err = RealizedVolEngine::from_config(cfg).unwrap_err();
    assert!(
        err.contains("forecast RV is not enabled"),
        "unexpected validation error: {err}"
    );
}

#[test]
fn engine_config_validation_rejects_mixed_enabled_quorum_source_contracts() {
    let mut mixed_contracts = config(&[SOURCE_A, SOURCE_B]);
    mixed_contracts.sources[1].source_class = RealizedVolSourceClass::Trade;
    mixed_contracts.sources[1].sample_kind = RealizedVolSampleKind::Trade;

    let error = RealizedVolEngine::from_config(mixed_contracts)
        .expect_err("enabled quorum-counting sources must share one class/kind contract");

    assert!(
        error.contains("source_class") || error.contains("sample_kind"),
        "unexpected validation error: {error}"
    );
}

#[test]
fn invalid_config_snapshot_uses_explicit_invalid_config_blocker() {
    let snapshot = RealizedVolSnapshot::invalid_config(
        "<surface_id>",
        4_000,
        RealizedVolAggregation::UpperQuantile { quantile: 1.0 },
        31_536_000.0,
        "<config_fingerprint>",
    );

    assert!(!snapshot.ready);
    assert_eq!(snapshot.annualized_realized_vol_decimal, None);
    assert_eq!(
        snapshot.blocked_reasons,
        vec![RealizedVolBlockReason::InvalidConfig]
    );
}

#[test]
fn realized_volatility_block_reason_contract_is_exhaustive() {
    assert_eq!(
        RealizedVolBlockReason::ALL,
        &[
            RealizedVolBlockReason::InvalidConfig,
            RealizedVolBlockReason::QuorumNotReady,
            RealizedVolBlockReason::SourceStale,
            RealizedVolBlockReason::CoverageBelowMinimum,
            RealizedVolBlockReason::InterSampleGapExceeded,
            RealizedVolBlockReason::SourceClassMismatch,
            RealizedVolBlockReason::SampleKindMismatch,
            RealizedVolBlockReason::CrossSourceDispersion,
            RealizedVolBlockReason::AnnualizationBasisInvalid,
            RealizedVolBlockReason::NotWarm,
        ]
    );
}
