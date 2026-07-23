#![cfg(test)]

use super::*;
use crate::bolt_v3_capital_reservation::ReservationRejectionReason;
use crate::bolt_v3_config::{
    BoltV3RootConfig, ClientBlock, DataClientReadinessProbeBlock,
    DataClientReadinessProbeMarketDataKind, DataClientReadinessProbeQuoteTargetBlock,
    DataClientReadinessProbeQuoteTargetSource, DataInstrumentBlock,
    RealizedVolatilityAggregationBlock, RealizedVolatilityPolicyBlock,
    RealizedVolatilitySampleKindBlock, RealizedVolatilitySourceBlock,
    RealizedVolatilitySourceClassBlock, RealizedVolatilitySurfaceBlock, StrategyArchetypeKey,
};
use crate::bolt_v3_iv::config::IvRootConfig;
use crate::bolt_v3_iv::error::IvRejectReason;
use crate::bolt_v3_loss_governor::{LossSnapshot, LossSourceObservationTimestamps};
use crate::bolt_v3_operator_health::{
    BoltV3InputHealth, BoltV3OperatorHealthStatus, BoltV3OperatorHealthSurface,
    BoltV3RejectObserverHealth, BoltV3SettlementHealth, BoltV3VenueTruthHealth,
};
use crate::bolt_v3_providers::hyperliquid::{
    ResolvedBoltV3HyperliquidSecrets, hyperliquid_live_submit_signer_fingerprint,
};
use crate::bolt_v3_providers::hyperliquid_artifacts::{
    HyperliquidLiveSubmitApprovalInput, HyperliquidLiveSubmitOrderLimits,
    HyperliquidProductSubmitProofBinding, write_hyperliquid_live_submit_approval_artifact,
};
use nautilus_core::{UUID4, UnixNanos};
use nautilus_model::data::{BookOrder, OrderBookDelta, OrderBookDeltas};
use nautilus_model::enums::{
    AccountType, BookAction, CurrencyType, OrderSide, TimeInForce, TradingState,
};
use nautilus_model::events::{AccountState, OrderAccepted, OrderEventAny, OrderSubmitted};
use nautilus_model::identifiers::{
    AccountId, ClientId, ClientOrderId, TraderId, Venue, VenueOrderId,
};
use nautilus_model::orders::{LimitOrder, MarketOrder, OrderAny};
use nautilus_model::types::{AccountBalance, Currency, Money, Price, Quantity};
use rust_decimal::Decimal;
use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_TEST_CATALOG_ID: AtomicU64 = AtomicU64::new(0);
const PRODUCER_STOPPED_EVENT: &str = "producer_stopped";
const DRAIN_SHUTDOWN_EVENT: &str = "drain_shutdown";

struct RecordingProducerStopper {
    events: Arc<Mutex<Vec<&'static str>>>,
}

impl BoltV3DecisionEvidenceProducerStopper for RecordingProducerStopper {
    fn stop_before_decision_evidence_drain(
        self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()>>> {
        Box::pin(async move {
            self.events
                .lock()
                .expect("test event log lock should be available")
                .push(PRODUCER_STOPPED_EVENT);
        })
    }
}

mod data_client_probe;
mod fixtures;
mod governance_mode;
mod iv_runtime;
mod live_node_config;
mod provider_approvals;
mod startup_rebuild;
mod strategy_free_probe;
mod transport_scope;

use fixtures::*;

#[test]
fn live_operator_health_surface_renders_poisoned_reject_feed_as_degraded() {
    let writer = Arc::new(DecisionEvidenceRecorder::recording());
    let feed = Arc::new(Mutex::new(BoltV3OrderRejectObserverFeed::new(
        writer.clone(),
        AccountId::from("ACCOUNT-POISON"),
    )));
    let poison_feed = feed.clone();
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
        let _guard = poison_feed
            .lock()
            .expect("test should acquire feed lock before poisoning it");
        panic!("poison reject feed");
    }));
    let submit_admission = BoltV3SubmitAdmissionState::new(writer.clone());

    let surface = live_operator_health_surface(
        Some(&feed),
        &submit_admission,
        false,
        0,
        None,
        BoltV3SettlementHealth::nominal(),
        &writer,
    );

    assert_eq!(
        surface.reject_observer.status,
        BoltV3OperatorHealthStatus::Degraded
    );
    assert_eq!(
        surface.reject_observer.read_error.as_deref(),
        Some(OPERATOR_HEALTH_REJECT_OBSERVER_READ_ERROR)
    );
}

#[test]
fn live_operator_health_surface_renders_poisoned_submit_admission_as_venue_truth_read_error() {
    let writer = Arc::new(DecisionEvidenceRecorder::recording());
    let temp = tempfile::tempdir().expect("tempdir should create");
    let loaded = loaded_config_with_submit_sizer_recovery(temp.path());
    let capital_admission = capital_admission_config_from_loaded(&loaded)
        .expect("fixture capital admission config should parse")
        .expect("fixture should configure venue-truth capital admission");
    let submit_admission =
        BoltV3SubmitAdmissionState::new_with_capital_admission(writer.clone(), capital_admission);
    let poisoned = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        submit_admission.poison_inner_for_test();
    }));
    assert!(poisoned.is_err());

    let surface = live_operator_health_surface(
        None,
        &submit_admission,
        true,
        0,
        None,
        BoltV3SettlementHealth::nominal(),
        &writer,
    );

    assert_eq!(
        surface.venue_truth.status,
        BoltV3OperatorHealthStatus::Degraded
    );
    assert_eq!(
        surface.venue_truth.read_error.as_deref(),
        Some(OPERATOR_HEALTH_SUBMIT_ADMISSION_READ_ERROR)
    );
}

#[test]
fn live_operator_health_surface_reads_midrun_observation_poison_from_recorder() {
    use crate::bolt_v3_current_evidence::{
        EvidenceRequoteLeg, ObservationRecordOutcome, RequoteActionCostClass,
        RequoteThrottleBlockReason, RequoteThrottleBound, RequoteThrottleObservationFact,
    };

    let writer = Arc::new(DecisionEvidenceRecorder::recording());
    let submit_admission = BoltV3SubmitAdmissionState::new(writer.clone());
    writer.fail_observation_writes();
    let outcome = writer.record_requote_throttle_observation(RequoteThrottleObservationFact {
        strategy_id: "strategy-1".to_string(),
        family_key: "family-1".to_string(),
        market_id: Some("market-1".to_string()),
        leg: EvidenceRequoteLeg::Yes,
        now_ms: 6,
        observed_at_ns: 7,
        action_cost_class: RequoteActionCostClass::CancelResubmit,
        block_reason: RequoteThrottleBlockReason::RequoteBudgetExhausted,
        bound_by: RequoteThrottleBound::RestCallWindow,
        submit_commands_in_window: 2,
        submit_command_cap: 3,
        submit_window_ms: 1_000,
        rest_cost_in_window: 4,
        rest_cap_per_minute: 5,
        rest_window_ms: 60_000,
        min_interval_ms: 100,
    });
    assert!(matches!(
        outcome,
        ObservationRecordOutcome::FailureReported(_)
    ));

    let surface = live_operator_health_surface(
        None,
        &submit_admission,
        false,
        0,
        None,
        BoltV3SettlementHealth::nominal(),
        &writer,
    );

    assert_eq!(
        surface.decision_evidence_observation.status,
        BoltV3OperatorHealthStatus::Degraded
    );
    assert!(
        surface
            .decision_evidence_observation
            .poison_cause
            .as_deref()
            .is_some_and(|cause| cause.contains("commit indeterminate"))
    );
}

#[test]
fn live_input_health_accumulator_starts_unobserved_then_tracks_source_transitions() {
    let chainlink_source = BoltV3MissingInputSource {
        strategy_instance_id: "binary-edge-taker".to_string(),
        source_id: "chainlink_primary".to_string(),
        asset: "BTC".to_string(),
        provider: "chainlink_ws".to_string(),
        provider_instrument: "BTC-USD.CHAINLINK".to_string(),
        reason: "initial".to_string(),
    };
    let polyresearch_source = BoltV3MissingInputSource {
        strategy_instance_id: "binary-edge-taker".to_string(),
        source_id: "polyresearch_backup".to_string(),
        asset: "BTC".to_string(),
        provider: "polyresearch".to_string(),
        provider_instrument: "BTC".to_string(),
        reason: "initial".to_string(),
    };
    let sources_by_client = BTreeMap::from([
        (
            "chainlink_reference".to_string(),
            vec![chainlink_source.clone()],
        ),
        (
            "polyresearch_reference".to_string(),
            vec![polyresearch_source.clone()],
        ),
    ]);
    let mut accumulator = BoltV3LiveInputHealthAccumulator::new(2, &sources_by_client);

    assert_eq!(
        accumulator.snapshot().status,
        BoltV3OperatorHealthStatus::Unobserved
    );

    let after_chainlink_recovered =
        accumulator.apply_transition(BoltV3InputHealthSourceTransition {
            source: chainlink_source.clone(),
            missing: false,
        });

    assert_eq!(
        after_chainlink_recovered.status,
        BoltV3OperatorHealthStatus::MissingInput
    );
    assert_eq!(after_chainlink_recovered.observed_source_count, 1);
    assert_eq!(
        after_chainlink_recovered.missing_sources[0].source_id,
        "polyresearch_backup"
    );
    assert_eq!(
        after_chainlink_recovered.missing_sources[0].reason,
        OPERATOR_HEALTH_INPUT_SOURCE_UNOBSERVED_REASON
    );

    let mut stale_chainlink_source = chainlink_source;
    stale_chainlink_source.reason = "stale".to_string();
    let after_chainlink_stale = accumulator.apply_transition(BoltV3InputHealthSourceTransition {
        source: stale_chainlink_source,
        missing: true,
    });

    assert_eq!(
        after_chainlink_stale.status,
        BoltV3OperatorHealthStatus::MissingInput
    );
    assert_eq!(after_chainlink_stale.observed_source_count, 0);
    assert_eq!(after_chainlink_stale.missing_sources.len(), 2);
}

#[test]
fn poisoned_live_input_health_snapshot_fails_closed_as_unobserved() {
    let accumulator = Arc::new(Mutex::new(BoltV3LiveInputHealthAccumulator::new(
        1,
        &BTreeMap::new(),
    )));
    poison_mutex(&accumulator);

    assert_eq!(live_input_health_snapshot(&accumulator), None);

    let input_health = apply_live_input_health_transition(
        &accumulator,
        1,
        BoltV3InputHealthSourceTransition {
            source: BoltV3MissingInputSource {
                strategy_instance_id: "poisoned-input-health".to_string(),
                source_id: "poisoned-source".to_string(),
                asset: "BTC".to_string(),
                provider: "poisoned-provider".to_string(),
                provider_instrument: "BTC-USD.POISONED".to_string(),
                reason: "poisoned".to_string(),
            },
            missing: false,
        },
    );
    assert_eq!(input_health.status, BoltV3OperatorHealthStatus::Unobserved);
    assert_eq!(input_health.configured_source_count, 1);
    assert_eq!(input_health.observed_source_count, 0);
}

#[test]
fn live_input_health_sources_include_only_providers_with_transition_emitters() {
    let loaded = crate::bolt_v3_config::load_bolt_v3_config(std::path::Path::new(
        "tests/fixtures/bolt_v3/root.toml",
    ))
    .expect("fixture config should load");

    let sources_by_client = reference_current_price_live_input_sources_by_client(&loaded);
    let source_count = configured_reference_current_price_source_count(&sources_by_client);
    let sources = sources_by_client
        .values()
        .flat_map(|sources| sources.iter())
        .collect::<Vec<_>>();

    assert_eq!(source_count, sources.len());
    assert!(
        sources.iter().any(|source| source.provider
            == crate::bolt_v3_providers::chainlink_reference::REFERENCE_PRICE_PROVIDER_KEY),
        "fixture must include the Chainlink source that can emit live input-health transitions"
    );
    assert!(
        sources.iter().all(|source| source.provider
            != crate::bolt_v3_providers::polyresearch::REFERENCE_PRICE_PROVIDER_KEY),
        "providers without live input-health emitters must not be enrolled as live-health sources"
    );
}

#[test]
fn operator_health_transition_logger_dedupes_identical_and_emits_changed_surface() {
    let logger = BoltV3OperatorHealthTransitionLogger::new();
    let nominal = BoltV3OperatorHealthSurface::not_configured();
    let changed = BoltV3OperatorHealthSurface::from_parts(
        BoltV3RejectObserverHealth::unobserved(),
        BoltV3VenueTruthHealth::not_configured(),
        BoltV3InputHealth::not_configured(),
    );

    assert_eq!(
        logger.emit_surface(OPERATOR_HEALTH_REASON_LIVE_NODE_STARTUP, nominal.clone()),
        BoltV3OperatorHealthTransitionEmission::Emitted
    );
    assert_eq!(
        logger.emit_surface(OPERATOR_HEALTH_REASON_LIVE_NODE_STARTUP, nominal),
        BoltV3OperatorHealthTransitionEmission::Deduped
    );
    assert_eq!(
        logger.emit_surface(OPERATOR_HEALTH_REASON_LIVE_NODE_STARTUP, changed),
        BoltV3OperatorHealthTransitionEmission::Emitted
    );
}

fn loaded_config_with_strategy_archetypes(archetypes: &[&str]) -> LoadedBoltV3Config {
    #[derive(serde::Deserialize)]
    struct StrategyArchetypeFixture {
        strategy_archetype: StrategyArchetypeKey,
    }

    let mut loaded = crate::bolt_v3_config::load_bolt_v3_config(std::path::Path::new(
        "tests/fixtures/bolt_v3/root.toml",
    ))
    .expect("fixture config with registered strategies should load");
    let template = loaded
        .strategies
        .first()
        .expect("fixture should include a strategy")
        .clone();
    loaded.strategies = archetypes
        .iter()
        .map(|archetype| {
            let mut strategy = template.clone();
            let fixture: StrategyArchetypeFixture =
                toml::from_str(&format!("strategy_archetype = \"{archetype}\""))
                    .expect("test archetype key should deserialize");
            strategy.config.strategy_archetype = fixture.strategy_archetype;
            strategy
        })
        .collect();
    loaded
}

#[test]
fn settlement_health_is_nominal_for_edge_taker() {
    let loaded = loaded_config_with_strategy_archetypes(&["binary_oracle_edge_taker"]);
    assert_eq!(
        settlement_health_from_loaded(&loaded).status,
        BoltV3OperatorHealthStatus::Nominal
    );
}

#[test]
fn settlement_health_is_not_configured_for_maker_only() {
    let loaded = loaded_config_with_strategy_archetypes(&["binary_oracle_maker"]);
    let health = settlement_health_from_loaded(&loaded);
    assert_eq!(health.status, BoltV3OperatorHealthStatus::NotConfigured);
    assert!(!health.configured);
}

#[test]
fn settlement_health_is_not_configured_for_complete_set_only() {
    let loaded = loaded_config_with_strategy_archetypes(&["complete_set_arbitrage"]);
    let health = settlement_health_from_loaded(&loaded);
    assert_eq!(health.status, BoltV3OperatorHealthStatus::NotConfigured);
    assert!(!health.configured);
}

#[test]
fn settlement_health_is_nominal_for_mixed_capabilities() {
    let loaded = loaded_config_with_strategy_archetypes(&[
        "binary_oracle_maker",
        "binary_oracle_edge_taker",
        "complete_set_arbitrage",
    ]);
    assert_eq!(
        settlement_health_from_loaded(&loaded).status,
        BoltV3OperatorHealthStatus::Nominal
    );
}

#[test]
fn settlement_health_is_not_configured_without_strategies() {
    let loaded = loaded_config_with_strategy_archetypes(&[]);
    let unconfigured = settlement_health_from_loaded(&loaded);
    assert_eq!(
        unconfigured.status,
        BoltV3OperatorHealthStatus::NotConfigured
    );
    assert!(!unconfigured.configured);
}

#[test]
fn production_settlement_health_emitter_updates_state_before_reporting_surface() {
    let settlement_health = Arc::new(Mutex::new(BoltV3SettlementHealth::nominal()));
    let input_health_accumulator = Arc::new(Mutex::new(BoltV3LiveInputHealthAccumulator::new(
        0,
        &BTreeMap::new(),
    )));
    let reported = Arc::new(Mutex::new(
        Vec::<(&'static str, BoltV3SettlementHealth)>::new(),
    ));
    let recorded = reported.clone();
    let reported_health = settlement_health.clone();
    let emit_surface: Arc<
        dyn Fn(&'static str, Option<BoltV3InputHealth>) -> Result<()> + Send + Sync + 'static,
    > = Arc::new(move |reason, _| {
        let health = reported_health
            .lock()
            .expect("settlement health mutex poisoned")
            .clone();
        recorded
            .lock()
            .expect("settlement health report mutex poisoned")
            .push((reason, health));
        Ok(())
    });
    let emitter = build_settlement_health_transition_emitter(
        settlement_health.clone(),
        input_health_accumulator,
        emit_surface,
    );

    emitter(BoltV3SettlementHealthTransition {
        settlement_key: "settlement-key-1".to_string(),
        position_id: "position-1".to_string(),
        reason: "settlement_booking_terminal".to_string(),
    })
    .expect("production settlement health emission should succeed");

    let health = settlement_health
        .lock()
        .expect("settlement health mutex poisoned")
        .clone();
    assert_eq!(health.status, BoltV3OperatorHealthStatus::Degraded);
    assert_eq!(health.terminal_transition_count, 1);
    let reports = reported
        .lock()
        .expect("settlement health report mutex poisoned");
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0].0, stringify!(settlement_booking_terminal));
    assert_eq!(reports[0].1.status, BoltV3OperatorHealthStatus::Degraded);
    assert_eq!(reports[0].1.terminal_transition_count, 1);
}

#[test]
fn production_settlement_health_emitter_returns_context_for_poisoned_lock() {
    let settlement_health = Arc::new(Mutex::new(BoltV3SettlementHealth::nominal()));
    let poison_target = settlement_health.clone();
    let _ = std::thread::spawn(move || {
        let _guard = poison_target
            .lock()
            .expect("settlement health lock should initially be available");
        panic!("poison settlement health lock for differential");
    })
    .join();
    let input_health_accumulator = Arc::new(Mutex::new(BoltV3LiveInputHealthAccumulator::new(
        0,
        &BTreeMap::new(),
    )));
    let emitter = build_settlement_health_transition_emitter(
        settlement_health,
        input_health_accumulator,
        Arc::new(|_, _| Ok(())),
    );

    let error = emitter(BoltV3SettlementHealthTransition {
        settlement_key: "settlement-key-poisoned".to_string(),
        position_id: "position-poisoned".to_string(),
        reason: "settlement_booking_terminal".to_string(),
    })
    .expect_err("poisoned settlement health lock must be explicit");

    assert!(
        error
            .to_string()
            .contains("settlement health lock poisoned")
    );
}

#[test]
fn settlement_health_snapshot_returns_context_for_poisoned_lock() {
    let settlement_health = Arc::new(Mutex::new(BoltV3SettlementHealth::nominal()));
    poison_mutex(&settlement_health);

    let error = settlement_health_snapshot(&settlement_health)
        .expect_err("poisoned settlement health must remain an explicit read error");

    assert!(
        error
            .to_string()
            .contains("settlement health lock poisoned")
    );
}

#[test]
fn settlement_runtime_sink_panics_on_poisoned_capital_admission_feed() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let loaded = loaded_config_with_submit_sizer_recovery(temp.path());
    let runtime = build_bolt_v3_live_node_with(&loaded, |_| false, fake_bolt_v3_resolver)
        .expect("fixture v3 LiveNode should build");
    let feed = runtime
        .capital_admission_runtime_feed
        .as_ref()
        .expect("fixture should configure capital-admission feed")
        .clone();
    poison_mutex(&feed);
    let sink = BoltV3LiveSettlementRuntimeSink {
        loss_protection: None,
        capital_admission_runtime_feed: Some(feed),
    };

    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        sink.record_venue_truth_settlement(
            crate::bolt_v3_venue_truth::VenueTruthSettlementExplanation {
                settlement_key: "poisoned-settlement".to_string(),
                market_id: "poisoned-market".to_string(),
                product_id: "poisoned-product".to_string(),
                side: OrderSide::Sell,
                settled_quantity: Decimal::ONE,
                payout_per_share: Decimal::ONE,
                collateral_payout: Decimal::ONE,
            },
        )
        .expect("poisoned feed must panic before returning a result");
    }))
    .expect_err("poisoned capital-admission feed must panic");

    assert!(
        crate::panic_payload_message(panic.as_ref())
            .contains("capital admission runtime feed lock poisoned")
    );
}

#[test]
fn operator_health_transition_logger_survives_poisoned_cache_lock() {
    let logger = BoltV3OperatorHealthTransitionLogger::new();
    let poison_logger = logger.clone();
    let poisoned = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
        let _guard = poison_logger
            .last_surface
            .lock()
            .expect("test should acquire logger lock before poisoning it");
        panic!("poison operator health transition logger");
    }));
    assert!(poisoned.is_err());

    let emission = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        logger.emit_surface(
            OPERATOR_HEALTH_REASON_LIVE_NODE_STARTUP,
            BoltV3OperatorHealthSurface::not_configured(),
        )
    }));

    assert_eq!(
        emission.expect("poisoned logger lock must not panic"),
        BoltV3OperatorHealthTransitionEmission::LoggerLockPoisoned
    );
}

#[tokio::test]
async fn decision_evidence_shutdown_drain_stops_producers_first() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let producer_guards = RecordingProducerStopper {
        events: Arc::clone(&events),
    };

    drain_after_stopping_decision_evidence_producers(producer_guards, || {
        events
            .lock()
            .expect("test event log lock should be available")
            .push(DRAIN_SHUTDOWN_EVENT);
        Ok::<(), anyhow::Error>(())
    })
    .await
    .expect("test drain should succeed");

    assert_eq!(
        events
            .lock()
            .expect("test event log lock should be available")
            .as_slice(),
        [PRODUCER_STOPPED_EVENT, DRAIN_SHUTDOWN_EVENT]
    );
}

#[tokio::test]
async fn producer_stop_event_records_when_stop_future_completes() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let producer_guards = RecordingProducerStopper {
        events: Arc::clone(&events),
    };

    let stop_future = producer_guards.stop_before_decision_evidence_drain();
    assert!(
        events
            .lock()
            .expect("test event log lock should be available")
            .is_empty(),
        "producer stop must not be recorded before the stop future completes"
    );

    stop_future.await;

    assert_eq!(
        events
            .lock()
            .expect("test event log lock should be available")
            .as_slice(),
        [PRODUCER_STOPPED_EVENT]
    );
}

#[test]
fn shutdown_classifier_surfaces_drain_failure_after_clean_run_and_capture() {
    let error = classify_live_node_shutdown(
        Ok(()),
        Ok(()),
        Err(BoltV3LiveNodeError::DecisionEvidenceShutdownDrain(
            anyhow::anyhow!("drain failed"),
        )),
    )
    .expect_err("drain failure must be surfaced");

    assert!(matches!(
        error,
        BoltV3LiveNodeError::DecisionEvidenceShutdownDrain(_)
    ));
}

#[test]
fn shutdown_classifier_composes_runtime_failure_with_drain_failure() {
    let error = classify_live_node_shutdown(
        Err(BoltV3LiveNodeError::RuntimeCaptureShutdown(
            anyhow::anyhow!("capture failed"),
        )),
        Ok(()),
        Err(BoltV3LiveNodeError::DecisionEvidenceShutdownDrain(
            anyhow::anyhow!("drain failed"),
        )),
    )
    .expect_err("runtime and drain failures must compose");

    assert!(matches!(
        error,
        BoltV3LiveNodeError::RunAndDecisionEvidenceShutdownDrain { .. }
    ));
    assert!(error.to_string().contains(
        "LiveNode run, runtime-capture, or IV lifecycle stop failed and bolt-v3 decision evidence shutdown drain failed"
    ));
}

#[test]
fn shutdown_classifier_composes_iv_stop_failure_with_drain_failure() {
    let error = classify_live_node_shutdown(
        Ok(()),
        Err(BoltV3LiveNodeError::Run(anyhow::anyhow!(
            "iv lifecycle stop failed"
        ))),
        Err(BoltV3LiveNodeError::DecisionEvidenceShutdownDrain(
            anyhow::anyhow!("drain failed"),
        )),
    )
    .expect_err("IV stop and drain failures must compose");

    assert!(matches!(
        error,
        BoltV3LiveNodeError::RunAndDecisionEvidenceShutdownDrain { .. }
    ));
    assert!(error.to_string().contains(
        "LiveNode run, runtime-capture, or IV lifecycle stop failed and bolt-v3 decision evidence shutdown drain failed"
    ));
}
