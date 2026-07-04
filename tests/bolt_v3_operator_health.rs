use bolt_v2::{
    bolt_v3_kill_switch::{KillSwitchHaltTrigger, KillSwitchState},
    bolt_v3_operator_health::{
        BoltV3InputHealth, BoltV3OperatorHealthStatus, BoltV3VenueTruthHealth,
    },
    bolt_v3_reference_price_health::{
        ReferenceCurrentPriceHealthReport, ReferenceCurrentPriceSourceUpdateObservation,
    },
};

#[test]
fn input_health_marks_unobserved_reference_source_as_missing_input() {
    let report = ReferenceCurrentPriceHealthReport {
        targets: Vec::new(),
        clients: Vec::new(),
        source_update_observations: vec![
            ReferenceCurrentPriceSourceUpdateObservation {
                strategy_instance_id: "binary-oracle".to_string(),
                source_id: "chainlink_primary".to_string(),
                asset: "BTC".to_string(),
                provider: "chainlink".to_string(),
                provider_instrument: "BTC/USD".to_string(),
                status: "observed".to_string(),
                reason: "observed".to_string(),
                observed_ts_ms: Some(1_000),
                received_ts_ms: Some(1_005),
            },
            ReferenceCurrentPriceSourceUpdateObservation {
                strategy_instance_id: "binary-oracle".to_string(),
                source_id: "polyresearch_backup".to_string(),
                asset: "BTC".to_string(),
                provider: "polyresearch".to_string(),
                provider_instrument: "BTC".to_string(),
                status: "timed_out".to_string(),
                reason: "timed_out".to_string(),
                observed_ts_ms: None,
                received_ts_ms: None,
            },
        ],
    };

    let health = BoltV3InputHealth::from_reference_current_price_report(&report);

    assert_eq!(health.status, BoltV3OperatorHealthStatus::MissingInput);
    assert_eq!(health.configured_source_count, 2);
    assert_eq!(health.observed_source_count, 1);
    assert_eq!(health.missing_sources.len(), 1);
    assert_eq!(health.missing_sources[0].source_id, "polyresearch_backup");
    assert_eq!(health.missing_sources[0].reason, "timed_out");
}

#[test]
fn venue_truth_health_renders_divergence_trigger_as_halted() {
    let state = KillSwitchState::Halted {
        halt_id: "halt-001".to_string(),
        trigger: KillSwitchHaltTrigger::venue_truth_divergence(
            "polymarket_venue_truth_rest",
            1_000,
            "venue truth divergence: collateral_balance",
        ),
    };

    let health = BoltV3VenueTruthHealth::from_kill_switch_and_capital_state(&state, None);

    assert_eq!(health.status, BoltV3OperatorHealthStatus::Halted);
    assert_eq!(health.kill_switch_state, "Halted");
    assert_eq!(
        health.divergence.as_ref().map(|divergence| {
            (
                divergence.source.as_str(),
                divergence.source_timestamp_unix_nanos,
            )
        }),
        Some(("polymarket_venue_truth_rest", 1_000))
    );
}
