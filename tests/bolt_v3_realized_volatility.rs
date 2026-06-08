use bolt_v2::bolt_v3_realized_volatility::{
    RealizedVolAggregation, RealizedVolBlockReason, RealizedVolEngine, RealizedVolEngineConfig,
    RealizedVolObservation, RealizedVolSampleKind, RealizedVolSnapshot, RealizedVolSourceClass,
    RealizedVolSourceConfig, RealizedVolSourceRejectReason, RealizedVolSourceStatus,
};

const SURFACE_ID: &str = "<surface_id>";
const SOURCE_A: &str = "<SOURCE_ID_A>";
const SOURCE_B: &str = "<SOURCE_ID_B>";

fn source(source_id: &str) -> RealizedVolSourceConfig {
    RealizedVolSourceConfig {
        source_id: source_id.to_string(),
        data_client_id: "<DATA_CLIENT_ID>".to_string(),
        instrument_id: "<INSTRUMENT_ID>.<DATA_CLIENT_ID>".to_string(),
        source_class: RealizedVolSourceClass::SpotQuote,
        sample_kind: RealizedVolSampleKind::Midpoint,
        enabled: true,
        counts_toward_quorum: true,
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
        max_event_receive_lag_ms: 250,
        max_inter_sample_gap_ms: 2_000,
        min_coverage_ratio: 0.75,
        max_cross_source_dispersion: 0.50,
        seconds_per_annum: 31_536_000.0,
        aggregation: RealizedVolAggregation::UpperQuantile { quantile: 1.0 },
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

fn observe_path(engine: &mut RealizedVolEngine, source_id: &str, prices: &[f64]) {
    for (index, price) in prices.iter().enumerate() {
        let ts_ms = (index as u64 + 1) * 1_000;
        assert!(engine.observe(observation(source_id, *price, ts_ms)));
    }
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
fn fixed_grid_coverage_is_required_before_source_is_ready() {
    let mut engine = RealizedVolEngine::from_config(config(&[SOURCE_A])).unwrap();
    assert!(engine.observe(observation(SOURCE_A, 100.0, 1_000)));
    assert!(engine.observe(observation(SOURCE_A, 104.0, 4_000)));

    let snapshot = engine.snapshot_at(4_000);

    assert!(!snapshot.ready);
    assert!(
        snapshot
            .blocked_reasons
            .contains(&RealizedVolBlockReason::CoverageBelowMinimum)
    );
    assert!(
        snapshot
            .blocked_reasons
            .contains(&RealizedVolBlockReason::QuorumNotReady)
    );
    assert_eq!(
        snapshot.source_diagnostics[0].status,
        RealizedVolSourceStatus::Waiting
    );
    assert!(snapshot.source_diagnostics[0].coverage_ratio < 0.75);
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
    assert_eq!(disabled.status, RealizedVolSourceStatus::Rejected);
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
    assert_eq!(disabled.status, RealizedVolSourceStatus::Waiting);
    assert_eq!(disabled.last_rejected_reason, None);
}

#[test]
fn observation_validation_rejects_timestamp_and_lag_violations() {
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
        (
            RealizedVolObservation {
                event_ts_ms: 2_000,
                recv_ts_ms: 1_999,
                ..observation(SOURCE_A, 101.0, 2_000)
            },
            RealizedVolSourceRejectReason::ReceiveBeforeEvent,
        ),
        (
            RealizedVolObservation {
                event_ts_ms: 2_000,
                recv_ts_ms: 2_500,
                ..observation(SOURCE_A, 101.0, 2_000)
            },
            RealizedVolSourceRejectReason::EventReceiveLagExceeded,
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
}

#[test]
fn flat_valid_source_publishes_zero_realized_volatility() {
    let mut engine = RealizedVolEngine::from_config(config(&[SOURCE_A])).unwrap();
    observe_path(&mut engine, SOURCE_A, &[100.0, 100.0, 100.0, 100.0]);

    let snapshot = engine.snapshot_at(4_000);

    assert!(snapshot.ready);
    assert_eq!(snapshot.annualized_realized_vol_decimal, Some(0.0));
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
