#![cfg(test)]

use super::shared_fixture::{unique_log_capture_strategy_id, with_captured_strategy_logs};
use super::*;
use crate::bolt_v3_binary_outcome_edge::BinaryOutcomeEdgeBlockReason;

const TEST_PRICING_SNAPSHOT_REFERENCE_STEP_MS: u64 = 100;
const TEST_PRICING_SNAPSHOT_REFERENCE_PRICE_STEP: f64 = 2.0;
const TEST_PRICING_SNAPSHOT_FAST_WEIGHT: f64 = 0.9;
const TEST_PRICING_SNAPSHOT_NEWER_REFERENCE_TS_MS: u64 = 1_100;
const TEST_PRICING_SNAPSHOT_STALE_REFERENCE_TS_MS: u64 =
    TEST_PRICING_SNAPSHOT_NEWER_REFERENCE_TS_MS - TEST_PRICING_SNAPSHOT_REFERENCE_STEP_MS;
const TEST_PRICING_SNAPSHOT_FRESH_SIGNAL_TS_MS: u64 =
    TEST_PRICING_SNAPSHOT_NEWER_REFERENCE_TS_MS + TEST_PRICING_SNAPSHOT_REFERENCE_STEP_MS;
const TEST_PRICING_SNAPSHOT_NEWER_REFERENCE_PRICE: f64 = 3_101.0;
const TEST_PRICING_SNAPSHOT_STALE_REFERENCE_PRICE: f64 =
    TEST_PRICING_SNAPSHOT_NEWER_REFERENCE_PRICE - TEST_PRICING_SNAPSHOT_REFERENCE_PRICE_STEP;
const TEST_PRICING_SNAPSHOT_FRESH_SIGNAL_PRICE: f64 = TEST_PRICING_SNAPSHOT_NEWER_REFERENCE_PRICE;
const TEST_PRICING_SNAPSHOT_MISMATCHED_STALE_REFERENCE_PRICE: f64 =
    TEST_PRICING_SNAPSHOT_REFERENCE_PRICE_STEP;

struct PriceSensitiveEntryFeeProvider;

const RV_CLOCK_DOMAIN_AMENDMENT_STALE_WALL_MS: u64 = 3_201;
#[derive(Clone, Copy, Debug)]
enum RvClockDomainAmendmentSnapshot {
    Ready(u64),
    NotReady(u64),
}

const RV_CLOCK_DOMAIN_AMENDMENT_CASES: [(RvClockDomainAmendmentSnapshot, u64, bool); 4] = [
    (RvClockDomainAmendmentSnapshot::Ready(1_200), 1_200, true),
    (RvClockDomainAmendmentSnapshot::Ready(1_201), 1_200, false),
    (RvClockDomainAmendmentSnapshot::Ready(1_200), 1_701, false),
    (
        RvClockDomainAmendmentSnapshot::NotReady(1_200),
        1_200,
        false,
    ),
];

fn rv_clock_domain_amendment_ready_entry() -> BinaryOracleEdgeTaker {
    let mut strategy = ready_to_trade_strategy_with_live_fees(Decimal::ZERO, Decimal::ZERO);
    rv_clock_domain_amendment_configure_surface(&mut strategy);
    rv_clock_domain_amendment_prepare_non_rv_inputs(&mut strategy);
    register_test_strategy_with_active_instruments(&mut strategy);
    assert_rv_clock_domain_amendment_non_rv_entry_gate_open(&strategy);
    strategy
}

fn rv_clock_domain_amendment_prepare_non_rv_inputs(strategy: &mut BinaryOracleEdgeTaker) {
    strategy.active.last_reference_ts_ms = Some(RV_CLOCK_DOMAIN_AMENDMENT_STALE_WALL_MS);
    strategy.pricing.set_selected_pricing_spot(Some(fast_spot(
        "bybit",
        3_100.5,
        RV_CLOCK_DOMAIN_AMENDMENT_STALE_WALL_MS,
    )));
}

fn assert_rv_clock_domain_amendment_non_rv_entry_gate_open(strategy: &BinaryOracleEdgeTaker) {
    assert!(
        strategy
            .entry_gate_decision_at(RV_CLOCK_DOMAIN_AMENDMENT_STALE_WALL_MS)
            .blocked_by
            .is_empty(),
        "the amendment fixture's ordinary non-RV entry gate must be open before testing RV receive-time classification"
    );
}

fn rv_clock_domain_amendment_configure_surface(strategy: &mut BinaryOracleEdgeTaker) {
    let surface_id = strategy.config.realized_volatility_surface_id.clone();
    let mut surfaces = std::collections::BTreeMap::new();
    surfaces.insert(
        surface_id.clone(),
        crate::bolt_v3_realized_volatility::RealizedVolEngineConfig {
            surface_id,
            window_ms: 4_000,
            sampling_interval_ms: 1_000,
            min_ready_sources: 1,
            max_source_age_ms: 500,
            max_inter_sample_gap_ms: 2_000,
            min_coverage_ratio: 0.75,
            max_cross_source_dispersion: 0.50,
            seconds_per_annum: 31_536_000.0,
            aggregation:
                crate::bolt_v3_realized_volatility::RealizedVolAggregation::UpperQuantile {
                    quantile: 1.0,
                },
            estimator: crate::bolt_v3_realized_volatility::RealizedVolEstimatorConfig::measured(),
            sources: vec![
                crate::bolt_v3_realized_volatility::RealizedVolSourceConfig {
                    source_id: "rv_clock_domain_amendment_source".to_string(),
                    data_client_id: "rv_clock_domain_amendment_client".to_string(),
                    instrument_id: "RV-CLOCK-DOMAIN-AMENDMENT.TEST".to_string(),
                    source_class:
                        crate::bolt_v3_realized_volatility::RealizedVolSourceClass::SpotQuote,
                    sample_kind:
                        crate::bolt_v3_realized_volatility::RealizedVolSampleKind::Midpoint,
                    enabled: true,
                    counts_toward_quorum: true,
                    canonical_base_asset: "RVTEST".to_string(),
                    canonical_quote_asset: "USD".to_string(),
                },
            ],
        },
    );
    strategy.context = strategy
        .context
        .clone()
        .with_realized_volatility_surfaces(surfaces);
}

fn rv_clock_domain_amendment_set_snapshot(
    strategy: &mut BinaryOracleEdgeTaker,
    snapshot: RvClockDomainAmendmentSnapshot,
) {
    strategy.pricing.clear_latest_realized_vol_snapshot();
    let snapshot_receive_ms = match snapshot {
        RvClockDomainAmendmentSnapshot::Ready(receive_ms)
        | RvClockDomainAmendmentSnapshot::NotReady(receive_ms) => receive_ms,
    };
    strategy.pricing.seed_ready_realized_vol(
        Some("<SOURCE_ID>".to_string()),
        1.5,
        snapshot_receive_ms,
    );
    if matches!(snapshot, RvClockDomainAmendmentSnapshot::NotReady(_)) {
        let mut not_ready = strategy
            .pricing
            .latest_realized_vol_snapshot_for_surface(
                &strategy.config.realized_volatility_surface_id,
            )
            .expect("seeded snapshot must be present before marking it not ready")
            .clone();
        not_ready.ready = false;
        strategy.pricing.observe_realized_vol_snapshot(not_ready);
        assert_eq!(
            strategy.pricing.classify_realized_vol_gate(
                &strategy.config.realized_volatility_surface_id,
                Some(LocalReceiveMs::new(snapshot_receive_ms)),
                strategy.realized_volatility_max_source_age_ms(),
            ),
            BoltV3RvGateResult::RejectedNotReady,
            "the negative fixture must be a present RejectedNotReady snapshot"
        );
    }
}

impl FeeProvider for PriceSensitiveEntryFeeProvider {
    fn fee_bps(&self, _instrument_id: InstrumentId) -> Option<Decimal> {
        Some(Decimal::ZERO)
    }

    fn entry_fee_bps(&self, _instrument: &InstrumentAny, entry_price: Decimal) -> Option<Decimal> {
        if entry_price <= Decimal::new(55, 2) {
            Some(Decimal::from(5_000))
        } else {
            Some(Decimal::ZERO)
        }
    }

    fn max_entry_fee_bps(
        &self,
        _instrument: &InstrumentAny,
        _entry_price: Decimal,
    ) -> Option<Decimal> {
        Some(Decimal::from(5_000))
    }

    fn warm(&self, _instrument_id: InstrumentId) -> BoxFuture<'_, Result<()>> {
        async move { Ok(()) }.boxed()
    }
}

#[test]
fn rv_clock_domain_amendment_initial_uncertainty_uses_entry_receive_stamp() {
    for (snapshot_receive_ms, evaluation_receive_ms, expected_available) in
        RV_CLOCK_DOMAIN_AMENDMENT_CASES
    {
        let mut strategy = rv_clock_domain_amendment_ready_entry();
        rv_clock_domain_amendment_set_snapshot(&mut strategy, snapshot_receive_ms);
        let evaluation = strategy.entry_evaluation_for_receive_at(
            RV_CLOCK_DOMAIN_AMENDMENT_STALE_WALL_MS,
            EntryEvaluationReceiveContext::new(LocalReceiveMs::new(evaluation_receive_ms)),
        );

        assert_eq!(
            evaluation.uncertainty_band_probability.is_some(),
            expected_available,
            "initial uncertainty classification mismatch for snapshot={snapshot_receive_ms:?} evaluation_receive_ms={evaluation_receive_ms}: {evaluation:#?}"
        );
    }
}

#[test]
fn rv_clock_domain_amendment_sized_fee_adjustment_uses_entry_receive_stamp() {
    let strategy = rv_clock_domain_amendment_ready_entry();
    let evaluation = strategy.entry_evaluation_for_receive_at(
        RV_CLOCK_DOMAIN_AMENDMENT_STALE_WALL_MS,
        EntryEvaluationReceiveContext::new(LocalReceiveMs::new(1_200)),
    );

    assert_eq!(evaluation.selected_side, Some(OutcomeSide::Up));
    assert!(
        evaluation
            .sized_executable_edge
            .is_some_and(|edge| edge.trade_allowed)
    );
    assert!(evaluation.sized_notional.is_some_and(is_positive_finite));
}

#[test]
fn rv_clock_domain_amendment_resized_fee_adjustment_uses_entry_receive_stamp() {
    let mut admitted_resized = None;
    for deep_ask_cents in 51..=75 {
        let mut strategy = rv_clock_domain_amendment_ready_entry();
        strategy.config.order_notional_target = 10.0;
        strategy.config.maximum_position_notional = 10.0;
        strategy.config.risk_lambda = 0.5;
        strategy.config.sizing_ev_reference_bps = 500;
        strategy.config.vwap_depth_limit_bps = 5_000;
        strategy.config.book_impact_cap_bps = 5_000;
        strategy.config.edge_threshold_basis_points = 0;
        strategy.config.slippage_buffer_bps = 0;
        set_configured_books_depth(
            &mut strategy,
            &[
                (BookAction::Clear, OrderSide::Buy, 0.49, 100.0),
                (BookAction::Add, OrderSide::Buy, 0.49, 100.0),
                (BookAction::Add, OrderSide::Sell, 0.50, 4.0),
                (
                    BookAction::Add,
                    OrderSide::Sell,
                    deep_ask_cents as f64 / 100.0,
                    100.0,
                ),
            ],
        );
        let evaluation = strategy.entry_evaluation_for_receive_at(
            RV_CLOCK_DOMAIN_AMENDMENT_STALE_WALL_MS,
            EntryEvaluationReceiveContext::new(LocalReceiveMs::new(1_200)),
        );
        let Some(final_notional) = evaluation.sized_notional else {
            continue;
        };
        let Some(preliminary_ev) = evaluation.up_worst_case_ev_bps else {
            continue;
        };
        let Some(impact_cap_notional) = evaluation.book_impact_cap_notional else {
            continue;
        };
        let preliminary_notional = choose_robust_size(&RobustSizingInputs {
            expected_ev_per_notional: preliminary_ev / BPS_DENOMINATOR,
            ev_reference_per_notional: strategy.config.sizing_ev_reference_bps as f64
                / BPS_DENOMINATOR,
            risk_lambda: strategy.config.risk_lambda,
            order_notional_target: strategy.config.order_notional_target,
            maximum_position_notional: strategy.config.maximum_position_notional,
            impact_cap_notional,
        });
        if evaluation.selected_side == Some(OutcomeSide::Up)
            && (final_notional - preliminary_notional).abs()
                > notional_float_tolerance(preliminary_notional)
        {
            admitted_resized = Some(evaluation);
            break;
        }
    }
    assert!(
        admitted_resized.is_some(),
        "an actual resized branch must remain admitted with stale wall time and fresh evaluation receive time"
    );
}

#[test]
fn rv_clock_domain_amendment_log_and_skip_evidence_use_entry_receive_stamp() {
    for (snapshot_receive_ms, evaluation_receive_ms, expected_available) in
        RV_CLOCK_DOMAIN_AMENDMENT_CASES
    {
        let mut strategy = rv_clock_domain_amendment_ready_entry();
        rv_clock_domain_amendment_set_snapshot(&mut strategy, snapshot_receive_ms);
        let decision = strategy.entry_submission_decision_for_receive_at(
            RV_CLOCK_DOMAIN_AMENDMENT_STALE_WALL_MS,
            EntryEvaluationReceiveContext::new(LocalReceiveMs::new(evaluation_receive_ms)),
        );
        let fields = strategy
            .entry_evaluation_log_fields_at(RV_CLOCK_DOMAIN_AMENDMENT_STALE_WALL_MS, &decision);

        assert_eq!(fields.realized_vol.is_some(), expected_available);
        assert_eq!(
            fields.realized_vol_source_ts_ms.is_some(),
            expected_available,
            "log/skip classification mismatch for snapshot={snapshot_receive_ms:?} evaluation_receive_ms={evaluation_receive_ms}"
        );
    }
}

#[test]
fn rv_clock_domain_amendment_submit_evidence_uses_entry_receive_stamp() {
    for (snapshot_receive_ms, evaluation_receive_ms, expected_available) in
        RV_CLOCK_DOMAIN_AMENDMENT_CASES
    {
        let mut strategy = rv_clock_domain_amendment_ready_entry();
        rv_clock_domain_amendment_set_snapshot(&mut strategy, snapshot_receive_ms);
        let decision = strategy.entry_submission_decision_for_receive_at(
            RV_CLOCK_DOMAIN_AMENDMENT_STALE_WALL_MS,
            EntryEvaluationReceiveContext::new(LocalReceiveMs::new(evaluation_receive_ms)),
        );
        let price = Price::new(0.50, 2);
        let quantity = Quantity::new(strategy.config.order_notional_target, 2);

        let snapshot = strategy.entry_strategy_input_evidence_snapshot_at(
            RV_CLOCK_DOMAIN_AMENDMENT_STALE_WALL_MS,
            &decision,
            ClientOrderId::from("RV-CLOCK-DOMAIN-AMENDMENT"),
            &price,
            &quantity,
        );

        assert_eq!(
            snapshot.is_ok(),
            expected_available,
            "submit-evidence classification mismatch for snapshot={snapshot_receive_ms:?} evaluation_receive_ms={evaluation_receive_ms}: {snapshot:?}"
        );
    }
}

#[test]
fn rv_clock_domain_amendment_durable_skip_route_uses_entry_receive_context() {
    let evidence = Arc::new(RecordingSequencedDecisionEvidenceWriter::default());
    let submit_admission = Arc::new(
        crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new(evidence.clone()),
    );
    let mut strategy = ready_to_trade_strategy_with_decision_evidence_and_submit_admission(
        evidence.clone(),
        submit_admission,
    );
    rv_clock_domain_amendment_configure_surface(&mut strategy);
    rv_clock_domain_amendment_prepare_non_rv_inputs(&mut strategy);
    strategy.active.warmup_count = 0;
    let strategy_id = unique_log_capture_strategy_id("rv-context-skip");
    strategy.config.strategy_id = strategy_id.clone();

    let (result, logs) = with_captured_strategy_logs(&strategy_id, || {
        strategy.try_submit_entry_order_for_receive(
            RV_CLOCK_DOMAIN_AMENDMENT_STALE_WALL_MS,
            EntryEvaluationReceiveContext::new(LocalReceiveMs::new(1_200)),
        )
    });

    assert_eq!(result.expect("blocked entry must persist its skip"), None);
    let skips = evidence
        .events()
        .into_iter()
        .filter_map(|event| match event {
            RecordedDecisionEvidenceEvent::EntrySkip(skip) => Some(skip),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(skips.len(), 1);
    assert_eq!(skips[0].realized_vol.as_deref(), Some("1.5"));
    assert!(logs.iter().any(|(_, message)| {
        message.contains("binary_oracle_edge_taker entry evaluation:")
            && message.contains(&strategy_id)
            && message.contains("realized_vol=Some(1.5)")
    }));
}

#[test]
fn rv_clock_domain_amendment_actual_submit_route_uses_entry_receive_context() {
    let evidence = Arc::new(RecordingSequencedDecisionEvidenceWriter::default());
    let submit_admission = Arc::new(
        crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new(evidence.clone()),
    );
    let mut strategy = ready_to_trade_strategy_with_decision_evidence_and_submit_admission(
        evidence.clone(),
        submit_admission,
    );
    rv_clock_domain_amendment_configure_surface(&mut strategy);
    rv_clock_domain_amendment_prepare_non_rv_inputs(&mut strategy);
    set_shadow_order_execution_policy(&mut strategy);
    register_test_strategy_with_active_instruments(&mut strategy);
    assert_rv_clock_domain_amendment_non_rv_entry_gate_open(&strategy);
    let strategy_id = unique_log_capture_strategy_id("rv-context-submit");
    strategy.config.strategy_id = strategy_id.clone();

    let (submit_result, logs) = with_captured_strategy_logs(&strategy_id, || {
        strategy.try_submit_entry_order_for_receive(
            RV_CLOCK_DOMAIN_AMENDMENT_STALE_WALL_MS,
            EntryEvaluationReceiveContext::new(LocalReceiveMs::new(1_200)),
        )
    });
    let client_order_id = submit_result
        .expect("entry route must preserve the trigger receive context")
        .expect("fresh evaluation receive time must admit a shadow submit");
    let events = evidence.events();
    assert!(events.iter().any(|event| matches!(
        event,
        RecordedDecisionEvidenceEvent::StrategyInput(snapshot)
            if snapshot.client_order_id == client_order_id.to_string()
                && snapshot.realized_volatility == "1.5"
    )));
    assert!(
        events
            .iter()
            .any(|event| matches!(event, RecordedDecisionEvidenceEvent::OrderIntent(_)))
    );
    assert!(logs.iter().any(|(_, message)| {
        message.contains("binary_oracle_edge_taker entry evaluation:")
            && message.contains(&strategy_id)
            && message.contains("realized_vol=Some(1.5)")
    }));
}

#[test]
fn rv_clock_domain_amendment_book_route_uses_init_stamp_for_entry_gate() {
    let evidence = Arc::new(RecordingSequencedDecisionEvidenceWriter::default());
    let submit_admission = Arc::new(
        crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new(evidence.clone()),
    );
    let mut strategy = ready_to_trade_strategy_with_decision_evidence_and_submit_admission(
        evidence.clone(),
        submit_admission,
    );
    rv_clock_domain_amendment_configure_surface(&mut strategy);
    set_shadow_order_execution_policy(&mut strategy);
    let (cache, clock) = register_test_strategy_with_clock(&mut strategy);
    add_active_instruments_to_cache(&strategy, &cache);
    let lifecycle_now_ms: u64 = 1_200;
    let book_event_ms = 1_701;
    let book_receive_ms = 1_200;
    clock.borrow_mut().set_time(UnixNanos::from(
        lifecycle_now_ms.saturating_mul(NANOS_PER_MILLI_U64),
    ));
    let instrument_id = selected_entry_instrument(&strategy);
    let stamped_deltas = book_deltas_with_stamps(
        instrument_id,
        &[(BookAction::Update, OrderSide::Sell, 0.50, 5_000.0)],
        book_event_ms,
        book_receive_ms,
    );
    assert_ne!(stamped_deltas.ts_event, stamped_deltas.ts_init);

    strategy
        .on_book_deltas(&stamped_deltas)
        .expect("unequal-stamped book deltas must stay inside the actor loop");

    let snapshots = evidence
        .events()
        .into_iter()
        .filter_map(|event| match event {
            RecordedDecisionEvidenceEvent::StrategyInput(snapshot) => Some(snapshot),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(snapshots.len(), 1);
    assert_eq!(
        snapshots[0].realized_volatility_gate_result,
        Some(BoltV3RvGateResult::Accepted),
        "entry must use OrderBookDeltas.ts_init; the venue event stamp is deliberately stale"
    );
    assert_eq!(
        snapshots[0].realized_volatility_receive_watermark_ms,
        Some(LocalReceiveMs::new(book_receive_ms))
    );
}

#[test]
fn non_signal_quote_tick_does_not_update_reference_current_price_or_signal() {
    let mut strategy = test_strategy();

    strategy
        .on_quote(&quote_tick("REFERENCE.SOURCE", 100.0, 102.0, 1_200))
        .expect("non-signal quote should process without mutating pricing");

    assert_eq!(strategy.pricing.last_reference_current_price(), None);
    assert_eq!(strategy.pricing.selected_pricing_spot().cloned(), None);
    assert!(!strategy.pricing.lead_quality_policy_applied);
}

#[test]
fn signal_quote_tick_updates_pricing_from_configured_signal_data() {
    let mut strategy = test_strategy();

    strategy
        .pricing
        .observe_reference_current_price(&fast_spot("chainlink_primary", 101.0, 1_100));
    strategy
        .on_quote(&quote_tick("SIGNAL.SOURCE", 100.5, 102.5, 1_200))
        .expect("signal quote should process");

    assert_eq!(strategy.pricing.last_reference_current_price(), Some(101.0));
    assert_eq!(
        strategy.pricing.selected_pricing_spot().cloned(),
        Some(fast_spot_received(
            "signal_data_client",
            101.5,
            1_200,
            Some(1_200),
        ))
    );
    assert!(strategy.pricing.lead_quality_policy_applied);
}

#[test]
fn invalid_signal_quote_tick_clears_stale_pricing_state() {
    let mut strategy = test_strategy();
    strategy
        .pricing
        .observe_reference_current_price(&fast_spot("chainlink_primary", 101.0, 1_100));
    strategy
        .on_quote(&quote_tick("SIGNAL.SOURCE", 100.5, 102.5, 1_200))
        .expect("signal quote should seed pricing");
    assert!(strategy.pricing.selected_pricing_spot().is_some());
    assert_eq!(strategy.evidence_spot_price(), Some(101.5));
    assert!(!strategy.pricing.fast_venue_incoherent);

    strategy
        .on_quote(&invalid_quote_tick("SIGNAL.SOURCE", 1_300))
        .expect("invalid signal quote should fail closed");

    assert_eq!(strategy.pricing.selected_pricing_spot().cloned(), None);
    assert_eq!(strategy.evidence_spot_price(), None);
    assert!(strategy.pricing.fast_venue_incoherent);
    assert!(strategy.active.fast_venue_incoherent);
    assert!(strategy.pricing.lead_quality_policy_applied);
    assert_eq!(strategy.pricing.last_lead_gap_probability, None);
    assert_eq!(strategy.pricing.last_jitter_penalty_probability, None);
    assert_eq!(strategy.pricing.last_lead_agreement_corr, None);
}

#[test]
fn signal_quote_tick_does_not_warm_active_reference_state() {
    let mut strategy = test_strategy();
    let mut market = candidate_market("market-1", 1_000);
    market.price_to_beat = Some(3_100.0);
    strategy.apply_selection_snapshot(selection_snapshot(1_000, SelectionState::Active { market }));
    strategy
        .pricing
        .set_last_reference_fair_value(Some(3_101.0));

    strategy
        .on_quote(&quote_tick("SIGNAL.SOURCE", 3_102.0, 3_104.0, 1_200))
        .expect("signal quote should process");

    assert_eq!(strategy.active.interval_open, None);
    assert_eq!(strategy.active.last_reference_ts_ms, None);
    assert_eq!(strategy.active.warmup_count, INITIAL_COUNTER_U64);
    assert_eq!(
        strategy.pricing.selected_pricing_spot().cloned(),
        Some(fast_spot_received(
            "signal_data_client",
            3_103.0,
            1_200,
            Some(1_200),
        ))
    );
}

#[test]
fn non_reference_quote_tick_does_not_update_pricing() {
    let mut strategy = test_strategy();

    strategy
        .on_quote(&quote_tick("OTHER.SOURCE", 100.0, 102.0, 1_200))
        .expect("non-reference quote should be ignored");

    assert_eq!(strategy.pricing.last_reference_current_price(), None);
    assert_eq!(strategy.pricing.selected_pricing_spot().cloned(), None);
}

#[test]
fn pricing_state_requires_fast_spot_for_pricing_and_keeps_reference_separate() {
    let config = test_strategy().config.clone();
    let mut pricing = PricingState::from_config(&taker_pricing_config(&config));

    pricing.observe_reference_snapshot(
        &reference_tick(1_000, 3_100.0),
        config.lead_agreement_min_corr,
        config.lead_jitter_max_ms,
    );
    assert_eq!(pricing.spot_price(), None);
    assert_eq!(pricing.last_reference_current_price(), Some(3_100.0));

    let snapshot = ReferenceSnapshot {
        ts_ms: 1_100,
        topic: "platform.reference.test.spot".to_string(),
        fair_value: Some(3_101.0),
        confidence: 1.0,
        venues: vec![
            oracle_venue("reference", 1.0, 3_101.0, 1_100),
            orderbook_venue("bybit", 0.9, 3_102.0, 1_100),
        ],
    };
    pricing.observe_reference_snapshot(
        &snapshot,
        config.lead_agreement_min_corr,
        config.lead_jitter_max_ms,
    );
    assert_eq!(pricing.spot_price(), Some(3_102.0));
}

#[test]
fn pricing_state_reference_snapshot_rejects_stale_fair_value() {
    let config = test_strategy().config.clone();
    let mut pricing = PricingState::from_config(&taker_pricing_config(&config));

    pricing.observe_reference_snapshot(
        &reference_tick(
            TEST_PRICING_SNAPSHOT_NEWER_REFERENCE_TS_MS,
            TEST_PRICING_SNAPSHOT_NEWER_REFERENCE_PRICE,
        ),
        config.lead_agreement_min_corr,
        config.lead_jitter_max_ms,
    );
    pricing.observe_reference_snapshot(
        &reference_tick(
            TEST_PRICING_SNAPSHOT_STALE_REFERENCE_TS_MS,
            TEST_PRICING_SNAPSHOT_STALE_REFERENCE_PRICE,
        ),
        config.lead_agreement_min_corr,
        config.lead_jitter_max_ms,
    );

    assert_eq!(
        pricing.last_reference_current_price(),
        Some(TEST_PRICING_SNAPSHOT_NEWER_REFERENCE_PRICE)
    );
    assert_eq!(
        pricing.last_reference_current_price_ts_ms(),
        Some(TEST_PRICING_SNAPSHOT_NEWER_REFERENCE_TS_MS)
    );
}

#[test]
fn pricing_state_reference_snapshot_processes_signal_candidates_when_fair_value_is_stale() {
    let config = test_strategy().config.clone();
    let mut pricing = PricingState::from_config(&taker_pricing_config(&config));
    let signal_venue = std::any::type_name::<PricingState>();

    pricing.observe_reference_snapshot(
        &reference_tick(
            TEST_PRICING_SNAPSHOT_NEWER_REFERENCE_TS_MS,
            TEST_PRICING_SNAPSHOT_NEWER_REFERENCE_PRICE,
        ),
        config.lead_agreement_min_corr,
        config.lead_jitter_max_ms,
    );
    pricing.observe_reference_snapshot(
        &ReferenceSnapshot {
            ts_ms: TEST_PRICING_SNAPSHOT_STALE_REFERENCE_TS_MS,
            topic: std::any::type_name::<ReferenceSnapshot>().to_string(),
            fair_value: Some(TEST_PRICING_SNAPSHOT_MISMATCHED_STALE_REFERENCE_PRICE),
            confidence: 1.0,
            venues: vec![orderbook_venue(
                signal_venue,
                TEST_PRICING_SNAPSHOT_FAST_WEIGHT,
                TEST_PRICING_SNAPSHOT_FRESH_SIGNAL_PRICE,
                TEST_PRICING_SNAPSHOT_FRESH_SIGNAL_TS_MS,
            )],
        },
        config.lead_agreement_min_corr,
        config.lead_jitter_max_ms,
    );

    assert_eq!(
        pricing.last_reference_current_price(),
        Some(TEST_PRICING_SNAPSHOT_NEWER_REFERENCE_PRICE)
    );
    assert_eq!(
        pricing.last_reference_current_price_ts_ms(),
        Some(TEST_PRICING_SNAPSHOT_NEWER_REFERENCE_TS_MS)
    );
    assert_eq!(
        pricing.selected_pricing_spot().cloned(),
        Some(fast_spot(
            signal_venue,
            TEST_PRICING_SNAPSHOT_FRESH_SIGNAL_PRICE,
            TEST_PRICING_SNAPSHOT_FRESH_SIGNAL_TS_MS,
        ))
    );
    assert!(!pricing.fast_venue_incoherent);
}

#[test]
fn pricing_state_requires_reference_anchor_for_fast_spot_selection() {
    let config = test_strategy().config.clone();
    let mut pricing = PricingState::from_config(&taker_pricing_config(&config));

    pricing.observe_reference_snapshot(
        &ReferenceSnapshot {
            ts_ms: 1_000,
            topic: "platform.reference.test.spot".to_string(),
            fair_value: None,
            confidence: 1.0,
            venues: vec![orderbook_venue("bybit", 0.9, 3_102.0, 1_000)],
        },
        config.lead_agreement_min_corr,
        config.lead_jitter_max_ms,
    );

    assert_eq!(pricing.spot_price(), None);
    assert_eq!(pricing.last_lead_gap_probability, None);
    assert_eq!(pricing.last_jitter_penalty_probability, None);
    assert_eq!(pricing.last_lead_agreement_corr, None);
}

#[test]
fn pricing_state_applies_lead_quality_thresholds() {
    let mut config = test_strategy().config.clone();
    config.lead_agreement_min_corr = 0.9999;
    let mut pricing = PricingState::from_config(&taker_pricing_config(&config));

    let snapshot = ReferenceSnapshot {
        ts_ms: 1_000,
        topic: "platform.reference.test.spot".to_string(),
        fair_value: Some(3_100.0),
        confidence: 1.0,
        venues: vec![
            oracle_venue("reference", 1.0, 3_100.0, 1_000),
            orderbook_venue("bybit", 0.9, 3_102.0, 1_000),
        ],
    };

    pricing.observe_reference_snapshot(
        &snapshot,
        config.lead_agreement_min_corr,
        config.lead_jitter_max_ms,
    );

    assert!(pricing.selected_pricing_spot().is_none());
    assert!(pricing.fast_venue_incoherent);
    assert_eq!(pricing.spot_price(), None);
    assert_eq!(pricing.last_reference_current_price(), Some(3_100.0));
}

#[test]
fn pricing_state_clears_fast_spot_when_no_fast_venue_remains() {
    let config = test_strategy().config.clone();
    let mut pricing = PricingState::from_config(&taker_pricing_config(&config));

    pricing.observe_reference_snapshot(
        &ReferenceSnapshot {
            ts_ms: 1_000,
            topic: "platform.reference.test.spot".to_string(),
            fair_value: Some(3_100.0),
            confidence: 1.0,
            venues: vec![
                oracle_venue("reference", 1.0, 3_100.0, 1_000),
                orderbook_venue("bybit", 0.9, 3_102.0, 1_000),
            ],
        },
        config.lead_agreement_min_corr,
        config.lead_jitter_max_ms,
    );
    assert_eq!(pricing.spot_price(), Some(3_102.0));

    pricing.observe_reference_snapshot(
        &ReferenceSnapshot {
            ts_ms: 1_100,
            topic: "platform.reference.test.spot".to_string(),
            fair_value: Some(3_101.0),
            confidence: 1.0,
            venues: vec![oracle_venue("reference", 1.0, 3_101.0, 1_100)],
        },
        config.lead_agreement_min_corr,
        config.lead_jitter_max_ms,
    );

    assert!(pricing.selected_pricing_spot().is_none());
    assert_eq!(pricing.spot_price(), None);
    assert_eq!(pricing.last_reference_current_price(), Some(3_101.0));
}

#[test]
fn entry_evaluation_log_fields_fail_closed_without_fast_spot() {
    let mut strategy = ready_to_trade_strategy_with_live_fees(Decimal::ZERO, Decimal::ZERO);
    strategy.pricing.set_selected_pricing_spot(None);
    strategy
        .pricing
        .set_last_reference_fair_value(Some(3_101.0));
    strategy
        .pricing
        .seed_ready_realized_vol(Some("<SOURCE_ID>".to_string()), 2.5, 1_200);

    let submission = strategy.entry_submission_decision_at(1_200);
    let fields = strategy.entry_evaluation_log_fields_at(1_200, &submission);

    assert_eq!(fields.spot_venue_name, None);
    assert_eq!(fields.spot_price, None);
    assert_eq!(
        fields.pricing_blocked_by,
        vec![EntryPricingBlockReason::SpotPriceMissing]
    );
    assert_eq!(fields.realized_vol, Some(2.5));
    assert_eq!(
        fields.realized_vol_source_venue.as_deref(),
        Some("<SOURCE_ID>")
    );
    assert_eq!(fields.realized_vol_source_ts_ms, Some(1_200));
}

#[test]
fn entry_evaluation_blocks_when_realized_vol_is_not_ready() {
    let mut strategy = ready_to_trade_strategy_with_live_fees(Decimal::ZERO, Decimal::ZERO);
    strategy
        .pricing
        .set_selected_pricing_spot(Some(fast_spot("bybit", 3_101.0, 1_200)));
    strategy.pricing.clear_latest_realized_vol_snapshot();

    let decision = strategy.entry_evaluation_at(1_200);

    assert!(decision.gate.blocked_by.is_empty());
    assert_eq!(
        decision.pricing_blocked_by,
        vec![EntryPricingBlockReason::RealizedVolNotReady]
    );
}

#[test]
fn live_fair_probability_is_computed_from_strategy_state_once_vol_warms() {
    let mut strategy = ready_to_trade_strategy();
    strategy.pricing = PricingState::from_config(&taker_pricing_config(&strategy.config));

    for (ts_ms, fair_value, fast_spot_price) in [
        (1_000, 3_100.0, 3_100.0),
        (2_000, 3_101.0, 3_101.5),
        (3_000, 3_102.0, 3_103.0),
        (4_000, 3_103.0, 3_104.0),
    ] {
        strategy.observe_reference_snapshot(
            &ReferenceSnapshot {
                ts_ms,
                topic: "platform.reference.test.spot".to_string(),
                fair_value: Some(fair_value),
                confidence: 1.0,
                venues: vec![orderbook_venue("bybit", 0.9, fast_spot_price, ts_ms)],
            },
            LocalReceiveMs::new(ts_ms),
        );
    }
    strategy
        .pricing
        .seed_ready_realized_vol(Some("<SOURCE_ID>".to_string()), 2.5, 4_000);

    let fair_probability = strategy
        .current_fair_probability_up_at(4_000)
        .expect("warmed pricing state should produce fair probability");
    assert!(fair_probability.value() > 0.5);

    let decision = strategy.entry_evaluation_at(4_000);
    assert!(decision.pricing_blocked_by.is_empty());
}

#[test]
fn live_scaled_min_edge_uses_theta_scaler_near_expiry() {
    let mut strategy = ready_to_trade_strategy();
    strategy.config.edge_threshold_basis_points = 10;
    strategy.config.theta_decay_factor = 1.5;

    let early = strategy
        .current_scaled_min_edge_bps_at(1_000)
        .expect("theta-scaled threshold should compute");
    let late = strategy
        .current_scaled_min_edge_bps_at(591_000)
        .expect("theta-scaled threshold should compute");

    assert!((early - 10.0).abs() < 1e-9);
    assert!(late > early);
}

#[test]
fn reference_current_price_does_not_open_interval_without_source_bound_price_to_beat() {
    let mut strategy = test_strategy();
    strategy.apply_selection_snapshot(active_snapshot_with_start("MKT-1", 1_000));

    let quote = ReferenceQuote::try_new(
        "BTC",
        "reference",
        crate::bolt_v3_config::ReferencePriceProvider::new("chainlink_ws")
            .expect("test provider should be valid"),
        "BTC-USD.CHAINLINK_REFERENCE",
        3_101.0,
        None,
        None,
        1_000,
        1_000,
    )
    .expect("reference current price quote should construct");
    assert!(strategy.active.observe_reference_price_quote(&quote, false));

    assert_eq!(strategy.active.interval_open, None);
    assert_eq!(strategy.active.reference_current_price, Some(3_101.0));
    assert_eq!(strategy.active.last_reference_ts_ms, Some(1_000));
    assert_eq!(strategy.active.warmup_count, INITIAL_COUNTER_U64);
}

#[test]
fn entry_evaluation_uses_price_adjusted_fee_bps_not_cached_base_fee_rate() {
    let (mut strategy, fee_provider) =
        ready_to_trade_strategy_with_recording_fees(Decimal::from(1000), Decimal::from(1000));
    fee_provider.set_entry_fee_bps(
        "condition-MKT-1-MKT-1-UP.POLYMARKET",
        Decimal::from_str("511.111111111111").expect("test decimal should parse"),
    );
    fee_provider.set_entry_fee_bps(
        "condition-MKT-1-MKT-1-DOWN.POLYMARKET",
        Decimal::from_str("182.027027027027").expect("test decimal should parse"),
    );
    register_test_strategy_with_active_instruments(&mut strategy);

    let decision = strategy.entry_submission_decision_at(1_200);
    let fields = strategy.entry_evaluation_log_fields_at(1_200, &decision);

    assert!(
        fields
            .up_fee_bps
            .is_some_and(|value| (value - 511.111111111111).abs() < 1e-9),
        "up fee bps should use price-adjusted fee: {:?}",
        fields.up_fee_bps
    );
    assert!(
        fields
            .down_fee_bps
            .is_some_and(|value| (value - 182.027027027027).abs() < 1e-9),
        "down fee bps should use price-adjusted fee: {:?}",
        fields.down_fee_bps
    );
}

#[test]
fn task4_lead_arbitration_uses_composite_score_over_fixed_precedence() {
    let candidates = vec![
        lead_signal("younger_but_weaker", 10, 10, 0.81, 1.0, 0.01),
        lead_signal("older_but_stronger", 20, 10, 0.99, 4.0, 0.01),
    ];

    let selected =
        arbitrate_lead_reference(&candidates, 0.80, 25).expect("winner should be eligible");

    assert_eq!(selected.venue_name, "older_but_stronger");
}

#[test]
fn task4_lead_arbitration_uses_reference_when_no_fast_venue_is_eligible() {
    let candidates = vec![
        lead_signal("too_noisy", 20, 300, 0.95, 4.0, 0.01),
        lead_signal("disagrees", 20, 20, 0.79, 4.0, 0.01),
        lead_signal("weightless", 20, 20, 0.95, 0.0, 0.01),
    ];

    let selected = arbitrate_lead_reference(&candidates, 0.80, 250);

    assert!(selected.is_none());
}

#[test]
fn task4_lead_arbitration_fails_closed_on_exact_score_tie() {
    let candidates = vec![
        lead_signal("lighter", 10, 10, 0.90, 2.0, 0.01),
        lead_signal("heavier", 10, 10, 0.90, 3.0, 0.01),
    ];

    let selected = arbitrate_lead_reference(&candidates, 0.80, 25);

    assert!(selected.is_none());
}

#[test]
fn reference_spot_spike_sets_cooldown_and_blocks_then_allows_entry() {
    let mut strategy = ready_to_trade_strategy();
    // Threshold from the test config fixture.
    assert_eq!(strategy.config.spike_guard_return_threshold, 0.05);
    assert_eq!(strategy.config.spike_guard_cooldown_secs, 5);

    // Seed a previous reference-spot observation so the next one has a baseline.
    strategy
        .pricing
        .set_selected_pricing_spot(Some(fast_spot("bybit", 100.0, 1_000)));

    // A jump from 100.0 -> 110.0 is a 10% single-step move, >= the 5% threshold.
    strategy.pricing.observe_signal_quote(
        &fast_spot("bybit", 110.0, 2_000),
        &taker_pricing_config(&strategy.config),
    );

    // Cooldown deadline = observed_ts (2_000ms) + 5s * 1_000ms = 7_000ms.
    assert_eq!(strategy.pricing.spike_until_ms, Some(7_000));

    // Entry is blocked while now_ms < spike_until_ms.
    assert!(
        strategy
            .entry_gate_decision_at(6_999)
            .blocked_by
            .contains(&EntryBlockReason::SpotSpikeCooldown),
        "entry must be blocked before the spike cooldown deadline"
    );
    // Boundary: at the deadline the cooldown has elapsed (now_ms < deadline is false).
    assert!(
        !strategy
            .entry_gate_decision_at(7_000)
            .blocked_by
            .contains(&EntryBlockReason::SpotSpikeCooldown),
        "entry must be allowed once now_ms reaches the spike cooldown deadline"
    );
    assert!(
        !strategy
            .entry_gate_decision_at(7_001)
            .blocked_by
            .contains(&EntryBlockReason::SpotSpikeCooldown),
        "entry must be allowed after the spike cooldown deadline"
    );
}

#[test]
fn sub_threshold_reference_spot_move_does_not_arm_spike_cooldown() {
    let mut strategy = ready_to_trade_strategy();
    strategy.pricing.spike_until_ms = None;
    strategy
        .pricing
        .set_selected_pricing_spot(Some(fast_spot("bybit", 100.0, 1_000)));

    // A 2% move (100.0 -> 102.0) is below the 5% threshold.
    strategy.pricing.observe_signal_quote(
        &fast_spot("bybit", 102.0, 2_000),
        &taker_pricing_config(&strategy.config),
    );

    assert_eq!(
        strategy.pricing.spike_until_ms, None,
        "sub-threshold move must not arm the spike cooldown"
    );
    assert!(
        !strategy
            .entry_gate_decision_at(2_001)
            .blocked_by
            .contains(&EntryBlockReason::SpotSpikeCooldown)
    );
}

#[test]
fn spike_detection_requires_a_valid_previous_observation() {
    let mut strategy = ready_to_trade_strategy();
    strategy.pricing.set_selected_pricing_spot(None);
    strategy.pricing.spike_until_ms = None;

    // First observation has no baseline; a spike cannot be inferred.
    strategy.pricing.observe_signal_quote(
        &fast_spot("bybit", 110.0, 2_000),
        &taker_pricing_config(&strategy.config),
    );

    assert_eq!(
        strategy.pricing.spike_until_ms, None,
        "no previous observation means no spike"
    );
}

#[test]
fn spike_cooldown_deadline_only_extends_never_retracts() {
    // The spike cooldown is a fail-closed safety gate: an out-of-order spike
    // quote carrying an earlier timestamp must never shorten an active
    // cooldown (which would prematurely re-enable entry during volatility).
    let mut strategy = ready_to_trade_strategy();

    // Pre-arm an active cooldown deadline at 7_000ms with a seeded baseline.
    strategy.pricing.spike_until_ms = Some(7_000);
    strategy
        .pricing
        .set_selected_pricing_spot(Some(fast_spot("bybit", 100.0, 1_000)));

    // Out-of-order spike: 100 -> 130 (30% >= 5% threshold) at an earlier ts
    // (1_500ms). Its naive deadline 1_500 + 5_000 = 6_500ms is before the
    // active 7_000ms and must not retract it.
    strategy.pricing.observe_signal_quote(
        &fast_spot("bybit", 130.0, 1_500),
        &taker_pricing_config(&strategy.config),
    );
    assert_eq!(
        strategy.pricing.spike_until_ms,
        Some(7_000),
        "an out-of-order spike must not shorten an active cooldown deadline"
    );

    // A later spike further into the future extends the deadline forward.
    // Reset the baseline so detection is independent of eligibility chaining.
    strategy
        .pricing
        .set_selected_pricing_spot(Some(fast_spot("bybit", 100.0, 1_000)));
    strategy.pricing.observe_signal_quote(
        &fast_spot("bybit", 130.0, 4_000),
        &taker_pricing_config(&strategy.config),
    );
    assert_eq!(
        strategy.pricing.spike_until_ms,
        Some(9_000),
        "a later spike (ts 4_000 + 5s) must extend the deadline to 9_000ms"
    );
}

#[test]
fn task5_exit_decision_uses_hold_favoring_hysteresis_and_holds_on_missing_inputs() {
    assert_eq!(
        evaluate_exit_decision(Some(12.0), Some(13.1), 1.0),
        ExitDecision::Exit
    );
    assert_eq!(
        evaluate_exit_decision(Some(12.0), Some(13.0), 1.0),
        ExitDecision::Hold
    );
    assert_eq!(
        evaluate_exit_decision(Some(12.0), Some(12.0), 1.0),
        ExitDecision::Hold
    );
    assert_eq!(
        evaluate_exit_decision(None, Some(10.0), 1.0),
        ExitDecision::Hold
    );
    assert_eq!(
        evaluate_exit_decision(Some(12.0), Some(f64::NAN), 1.0),
        ExitDecision::Hold
    );
    assert_eq!(
        evaluate_exit_decision(Some(12.0), Some(14.0), f64::NAN),
        ExitDecision::Hold
    );
}

#[test]
fn task6_entry_evaluation_blocks_when_realized_vol_is_not_ready() {
    let mut strategy = ready_to_trade_strategy_with_live_fees(Decimal::ZERO, Decimal::ZERO);
    strategy
        .pricing
        .set_selected_pricing_spot(Some(fast_spot("bybit", 3_101.0, 1_200)));
    strategy.pricing.clear_latest_realized_vol_snapshot();

    let decision = strategy.entry_evaluation_at(1_200);

    assert!(decision.gate.blocked_by.is_empty());
    assert_eq!(
        decision.pricing_blocked_by,
        vec![EntryPricingBlockReason::RealizedVolNotReady]
    );
    assert_eq!(decision.selected_side, None);
}

#[test]
fn task6_entry_evaluation_computes_both_side_evs_from_live_state() {
    let mut strategy = ready_to_trade_strategy_with_live_fees(Decimal::ZERO, Decimal::ZERO);
    register_test_strategy_with_active_instruments(&mut strategy);
    // Both-sided cheap book so each outcome is unambiguously tradeable: the #789
    // diffusion-grounded uncertainty band is nonzero even at market open, so a
    // knife-edge near-strike book would correctly wipe the marginal side.
    let cheap_outcome_price = UNIT_F64 / BPS_DENOMINATOR.sqrt();
    set_configured_books_depth(
        &mut strategy,
        &[
            (
                BookAction::Clear,
                OrderSide::Buy,
                cheap_outcome_price,
                BPS_DENOMINATOR,
            ),
            (
                BookAction::Add,
                OrderSide::Buy,
                cheap_outcome_price,
                BPS_DENOMINATOR,
            ),
            (
                BookAction::Add,
                OrderSide::Sell,
                cheap_outcome_price,
                BPS_DENOMINATOR,
            ),
        ],
    );
    strategy
        .pricing
        .set_selected_pricing_spot(Some(fast_spot("bybit", 3_100.4, 1_200)));
    strategy
        .pricing
        .seed_ready_realized_vol(Some("<SOURCE_ID>".to_string()), 2.5, 1_200);

    let decision = strategy.entry_evaluation_at(1_200);

    assert!(decision.gate.blocked_by.is_empty());
    assert!(decision.pricing_blocked_by.is_empty());
    assert!(
        decision
            .fair_probability_up
            .is_some_and(|value| value.value() > 0.5),
        "live pricing should infer an up edge from spot above strike"
    );
    assert!(decision.up_worst_case_ev_bps.is_some());
    assert!(decision.down_worst_case_ev_bps.is_some());
    assert!(
        decision
            .expected_ev_per_notional
            .is_some_and(|value| value > 0.0)
    );
    assert!(
        decision
            .book_impact_cap_notional
            .is_some_and(|value| value > 0.0)
    );
    assert!(decision.sized_notional.is_some_and(|value| value > 0.0));
    assert_eq!(decision.selected_side, Some(OutcomeSide::Up));
    // #618 regression pin on the LIVE evaluation path (not just the sizing
    // unit tests): the accepted notional must be the dollar-anchored robust
    // size of the final evaluated edge, never the EV fraction reinterpreted
    // as dollars.
    let sized_notional = decision
        .sized_notional
        .expect("sized notional asserted above");
    let supported_notional = choose_robust_size(&RobustSizingInputs {
        expected_ev_per_notional: decision
            .expected_ev_per_notional
            .expect("expected EV asserted above"),
        ev_reference_per_notional: strategy.config.sizing_ev_reference_bps as f64 / BPS_DENOMINATOR,
        risk_lambda: strategy.config.risk_lambda,
        order_notional_target: strategy.config.order_notional_target,
        maximum_position_notional: strategy.config.maximum_position_notional,
        impact_cap_notional: decision
            .book_impact_cap_notional
            .expect("impact cap asserted above"),
    });
    assert!(
        (sized_notional - supported_notional).abs() <= notional_float_tolerance(supported_notional),
        "live-path sized notional {sized_notional} must equal the robust size \
         {supported_notional} of the final evaluated edge: {decision:#?}"
    );
}

#[test]
fn executable_edge_blocks_when_best_touch_cannot_fill_exact_notional_inside_vwap_limit() {
    let mut strategy = ready_to_trade_strategy_with_live_fees(Decimal::ZERO, Decimal::ZERO);
    strategy.config.order_notional_target = 5.0;
    strategy.config.vwap_depth_limit_bps = 0;
    strategy.config.slippage_buffer_bps = 0;
    strategy.config.edge_threshold_basis_points = 0;
    strategy
        .pricing
        .set_selected_pricing_spot(Some(fast_spot("bybit", 3_120.0, 1_200)));
    strategy
        .pricing
        .seed_ready_realized_vol(Some("<SOURCE_ID>".to_string()), 2.5, 1_200);
    set_configured_books_depth(
        &mut strategy,
        &[
            (BookAction::Clear, OrderSide::Buy, 0.49, 100.0),
            (BookAction::Add, OrderSide::Buy, 0.49, 100.0),
            (BookAction::Add, OrderSide::Sell, 0.50, 2.0),
            (BookAction::Add, OrderSide::Sell, 0.70, 100.0),
        ],
    );

    let evaluation = strategy.entry_evaluation_at(1_200);

    assert_eq!(
        evaluation.pricing_blocked_by,
        vec![
            EntryPricingBlockReason::ExecutableEdgeUnavailable(
                OutcomeSide::Up,
                BinaryOutcomeEdgeBlockReason::InsufficientDepth
            ),
            EntryPricingBlockReason::ExecutableEdgeUnavailable(
                OutcomeSide::Down,
                BinaryOutcomeEdgeBlockReason::InsufficientDepth
            ),
        ]
    );
    assert_eq!(
        evaluation
            .up_executable_edge
            .as_ref()
            .and_then(|result| result.block_reason),
        Some(BinaryOutcomeEdgeBlockReason::InsufficientDepth)
    );
    assert_eq!(
        evaluation
            .down_executable_edge
            .as_ref()
            .and_then(|result| result.block_reason),
        Some(BinaryOutcomeEdgeBlockReason::InsufficientDepth)
    );
    assert_eq!(evaluation.up_worst_case_ev_bps, None);
    assert_eq!(evaluation.down_worst_case_ev_bps, None);
    assert_eq!(evaluation.selected_side, None);
}

#[test]
fn executable_edge_selects_tradeable_side_when_opposite_side_is_blocked() {
    let mut strategy = ready_to_trade_strategy_with_live_fees(Decimal::ZERO, Decimal::ZERO);
    register_test_strategy_with_active_instruments(&mut strategy);
    strategy.config.order_notional_target = 5.0;
    strategy.config.maximum_position_notional = 5.0;
    strategy.config.vwap_depth_limit_bps = 0;
    strategy.config.edge_threshold_basis_points = 0;
    strategy.config.slippage_buffer_bps = 0;
    strategy
        .pricing
        .set_selected_pricing_spot(Some(fast_spot("bybit", 3_120.0, 1_200)));
    strategy
        .pricing
        .seed_ready_realized_vol(Some("<SOURCE_ID>".to_string()), 2.5, 1_200);
    let up_instrument_id = strategy
        .instrument_id_for_side(OutcomeSide::Up)
        .expect("UP instrument should be configured");
    let down_instrument_id = strategy
        .instrument_id_for_side(OutcomeSide::Down)
        .expect("DOWN instrument should be configured");
    assert!(strategy.active.books.update_from_deltas(&book_deltas(
        up_instrument_id,
        &[
            (BookAction::Clear, OrderSide::Buy, 0.49, 100.0),
            (BookAction::Add, OrderSide::Buy, 0.49, 100.0),
            (BookAction::Add, OrderSide::Sell, 0.50, 100.0),
        ],
    )));
    assert!(strategy.active.books.update_from_deltas(&book_deltas(
        down_instrument_id,
        &[
            (BookAction::Clear, OrderSide::Buy, 0.49, 100.0),
            (BookAction::Add, OrderSide::Buy, 0.49, 100.0),
            (BookAction::Add, OrderSide::Sell, 0.50, 2.0),
            (BookAction::Add, OrderSide::Sell, 0.70, 100.0),
        ],
    )));

    let evaluation = strategy.entry_evaluation_at(1_200);

    assert!(
        evaluation.pricing_blocked_by.is_empty(),
        "nonselected side blockers must not veto a tradeable selected side: {evaluation:#?}"
    );
    assert_eq!(evaluation.selected_side, Some(OutcomeSide::Up));
    let down_edge = evaluation
        .down_executable_edge
        .expect("DOWN executable edge should still be evaluated");
    assert!(!down_edge.trade_allowed);
    assert_eq!(
        down_edge.block_reason,
        Some(BinaryOutcomeEdgeBlockReason::InsufficientDepth)
    );
    assert!(evaluation.up_worst_case_ev_bps.is_some());
    assert_eq!(evaluation.down_worst_case_ev_bps, None);
}

#[test]
fn sized_executable_edge_recomputes_uncertainty_band_from_sized_fee() {
    let mut strategy = test_strategy_with_fee_provider(Arc::new(PriceSensitiveEntryFeeProvider));
    configure_supported_market_quote_entry_order(&mut strategy);
    strategy.config.order_notional_target = 10.0;
    strategy.config.maximum_position_notional = 100.0;
    strategy.config.risk_lambda = 1.0;
    // A deliberately high EV reference scales the sized notional below the
    // 0.50-level depth so the sized probe's limit price drops into the
    // punitive <= 0.55 fee region and forces the sized re-evaluation block.
    strategy.config.sizing_ev_reference_bps = 10_000;
    strategy.config.book_impact_cap_bps = 5_000;
    strategy.config.vwap_depth_limit_bps = 5_000;
    strategy.config.edge_threshold_basis_points = i64::default();
    strategy.config.slippage_buffer_bps = 0;
    strategy.config.warmup_tick_count = 2;
    strategy.apply_selection_snapshot(active_snapshot_with_start("MKT-1", 1_000));
    strategy.active.price_to_beat = Some(3_100.0);
    strategy.active.interval_open = Some(3_100.0);
    strategy.active.warmup_count = 2;
    strategy.active.last_reference_ts_ms = Some(1_200);
    strategy.active.fast_venue_incoherent = false;
    strategy
        .pricing
        .set_selected_pricing_spot(Some(fast_spot("bybit", 3_500.0, 1_200)));
    strategy
        .pricing
        .seed_ready_realized_vol(Some("<SOURCE_ID>".to_string()), 1.5, 1_200);
    strategy.pricing.last_lead_gap_probability = Some(probability(0.0));
    strategy.pricing.last_jitter_penalty_probability = Some(probability(0.0));
    register_test_strategy_with_active_instruments(&mut strategy);

    let up_instrument_id = strategy
        .instrument_id_for_side(OutcomeSide::Up)
        .expect("UP instrument should be configured");
    let down_instrument_id = strategy
        .instrument_id_for_side(OutcomeSide::Down)
        .expect("DOWN instrument should be configured");
    assert!(strategy.active.books.update_from_deltas(&book_deltas(
        up_instrument_id,
        &[
            (BookAction::Clear, OrderSide::Buy, 0.49, 100.0),
            (BookAction::Add, OrderSide::Buy, 0.49, 100.0),
            (BookAction::Add, OrderSide::Sell, 0.50, 10.0),
            (BookAction::Add, OrderSide::Sell, 0.70, 100.0),
        ],
    )));
    assert!(strategy.active.books.update_from_deltas(&book_deltas(
        down_instrument_id,
        &[
            (BookAction::Clear, OrderSide::Buy, 0.49, 100.0),
            (BookAction::Add, OrderSide::Buy, 0.49, 100.0),
            (BookAction::Add, OrderSide::Sell, 0.90, 100.0),
        ],
    )));

    let evaluation = strategy.entry_evaluation_at(1_200);

    assert_eq!(
        evaluation.selected_side, None,
        "sized re-evaluation must block when the sized limit price has a wider fee band: {evaluation:#?}"
    );
    assert!(
        evaluation
            .up_executable_edge
            .is_some_and(|edge| edge.trade_allowed),
        "preliminary UP edge should remain visible when sized re-evaluation blocks: {evaluation:#?}"
    );
    assert_eq!(
        evaluation
            .sized_executable_edge
            .as_ref()
            .and_then(|edge| edge.block_reason),
        Some(BinaryOutcomeEdgeBlockReason::SpreadOrSlippageWipedEdge)
    );
    assert_eq!(
        evaluation.pricing_blocked_by,
        vec![EntryPricingBlockReason::ExecutableEdgeUnavailable(
            OutcomeSide::Up,
            BinaryOutcomeEdgeBlockReason::SpreadOrSlippageWipedEdge
        )],
        "sized selected-side threshold failure should surface as a pricing block"
    );
    // lead_gap and jitter are 0 and the recomputed sized fee contributes a 0.5
    // fee-uncertainty term, so the band is 0.5 plus the #789 diffusion-grounded
    // time term (realized_vol 1.5 * sqrt(T)) at seconds_to_expiry = 300 (snapshot
    // start 1_000ms, eval 1_200ms => 0 elapsed seconds). The prior inverted term
    // was exactly 0 at market open, which is why this used to assert band == 0.5.
    let expected_time_band = crate::bolt_v3_taker_updown_signal::time_uncertainty_probability(
        1.5,
        300,
        crate::bolt_v3_numeric::SECONDS_PER_YEAR_F64,
    )
    .expect("finite realized vol yields a time-uncertainty band")
    .value();
    let expected_band = 0.5 + expected_time_band;
    assert!(
        evaluation
            .uncertainty_band_probability
            .is_some_and(|band| (band.value() - expected_band).abs() < 1e-9),
        "final selected-side band should be the recomputed sized fee plus the diffusion time term: {evaluation:#?}"
    );
}

/// PR #623 review reproduction: a cliff-shaped ask book (thin cheap level,
/// expensive depth behind it) makes the sized re-evaluation oscillate — the
/// small first-pass size fills entirely at the cheap level, the EV jumps, the
/// resize saturates to the full target, and the final re-priced edge at the
/// full target is thin again. Acceptance must never keep a notional larger
/// than the robust size supported by the final re-priced edge.
#[test]
fn sized_acceptance_rejects_notional_unsupported_by_final_repriced_edge() {
    fn cliff_strategy() -> BinaryOracleEdgeTaker {
        let mut strategy = ready_to_trade_strategy_with_live_fees(Decimal::ZERO, Decimal::ZERO);
        register_test_strategy_with_active_instruments(&mut strategy);
        strategy.config.order_notional_target = 10.0;
        strategy.config.maximum_position_notional = 10.0;
        strategy.config.risk_lambda = 0.5;
        strategy.config.sizing_ev_reference_bps = 500;
        strategy.config.vwap_depth_limit_bps = 5_000;
        strategy.config.book_impact_cap_bps = 5_000;
        strategy.config.edge_threshold_basis_points = 0;
        strategy.config.slippage_buffer_bps = 0;
        strategy
            .pricing
            .set_selected_pricing_spot(Some(fast_spot("bybit", 3_120.0, 1_200)));
        strategy
            .pricing
            .seed_ready_realized_vol(Some("<SOURCE_ID>".to_string()), 2.5, 1_200);
        strategy
    }
    fn set_side_books(strategy: &mut BinaryOracleEdgeTaker, up_asks: &[(f64, f64)], up_bid: f64) {
        let up_instrument_id = strategy
            .instrument_id_for_side(OutcomeSide::Up)
            .expect("UP instrument should be configured");
        let down_instrument_id = strategy
            .instrument_id_for_side(OutcomeSide::Down)
            .expect("DOWN instrument should be configured");
        let mut up_deltas = vec![
            (BookAction::Clear, OrderSide::Buy, up_bid, 100.0),
            (BookAction::Add, OrderSide::Buy, up_bid, 100.0),
        ];
        for (price, shares) in up_asks {
            up_deltas.push((BookAction::Add, OrderSide::Sell, *price, *shares));
        }
        assert!(
            strategy
                .active
                .books
                .update_from_deltas(&book_deltas(up_instrument_id, &up_deltas))
        );
        assert!(strategy.active.books.update_from_deltas(&book_deltas(
            down_instrument_id,
            &[
                (BookAction::Clear, OrderSide::Buy, 0.48, 100.0),
                (BookAction::Add, OrderSide::Buy, 0.48, 100.0),
                (BookAction::Add, OrderSide::Sell, 0.90, 100.0),
            ],
        )));
    }

    // Phase A — calibrate the worst-case success probability this fixture's
    // pricing model produces, from a uniform tradeable book.
    let mut calibration = cliff_strategy();
    set_side_books(&mut calibration, &[(0.50, 100.0)], 0.49);
    let calibration_evaluation = calibration.entry_evaluation_at(1_200);
    assert_eq!(
        calibration_evaluation.selected_side,
        Some(OutcomeSide::Up),
        "calibration book must trade: {calibration_evaluation:#?}"
    );
    let fair_probability = calibration_evaluation
        .fair_probability_up
        .expect("calibration must expose the fair probability");
    let band_probability = calibration_evaluation
        .uncertainty_band_probability
        .expect("calibration must expose the uncertainty band");
    let worst_case_probability = fair_probability.narrowed(band_probability).value();
    assert!(
        (0.52..=0.97).contains(&worst_case_probability),
        "calibration produced an unusable worst-case probability \
         {worst_case_probability}: {calibration_evaluation:#?}"
    );

    // Phase B — build the cliff: a thin cheap ask the first-pass size fills
    // entirely, and a deep level priced so the full-target VWAP keeps a small
    // positive edge that supports far less than the full target.
    let target_notional = 10.0;
    let thin_notional = 2.0;
    let cheap_cents = ((worst_case_probability - 0.10) * 100.0).floor();
    let cheap = cheap_cents / 100.0;
    let mut deep = None;
    for cents in (cheap_cents as i64 + 1)..=99 {
        let price = cents as f64 / 100.0;
        let full_vwap =
            target_notional / (thin_notional / cheap + (target_notional - thin_notional) / price);
        let preliminary_ev = (worst_case_probability - full_vwap) / full_vwap;
        if preliminary_ev > 0.0005 && preliminary_ev < 0.0100 {
            deep = Some(price);
            break;
        }
    }
    let deep = deep.expect("a 2-decimal deep level must express the sizing cliff");

    let mut strategy = cliff_strategy();
    set_side_books(
        &mut strategy,
        &[(cheap, thin_notional / cheap), (deep, 100.0)],
        cheap - 0.01,
    );
    let evaluation = strategy.entry_evaluation_at(1_200);

    assert!(
        evaluation.sized_executable_edge.is_some(),
        "scenario must engage the sized re-evaluation loop: {evaluation:#?}"
    );
    if let (Some(sized_notional), Some(final_ev_per_notional), Some(impact_cap_notional)) = (
        evaluation.sized_notional,
        evaluation.expected_ev_per_notional,
        evaluation.book_impact_cap_notional,
    ) {
        let supported_notional = choose_robust_size(&RobustSizingInputs {
            expected_ev_per_notional: final_ev_per_notional,
            ev_reference_per_notional: strategy.config.sizing_ev_reference_bps as f64
                / BPS_DENOMINATOR,
            risk_lambda: strategy.config.risk_lambda,
            order_notional_target: strategy.config.order_notional_target,
            maximum_position_notional: strategy.config.maximum_position_notional,
            impact_cap_notional,
        });
        assert!(
            sized_notional <= supported_notional + notional_float_tolerance(supported_notional),
            "accepted sized_notional {sized_notional} exceeds the robust size \
             {supported_notional} supported by the final re-priced edge: {evaluation:#?}"
        );
    }
    assert_eq!(
        evaluation.selected_side, None,
        "the cliff book oscillates, so the entry must fail closed: {evaluation:#?}"
    );
    assert!(
        evaluation
            .pricing_blocked_by
            .contains(&EntryPricingBlockReason::SizedNotionalUnsupported(
                OutcomeSide::Up
            )),
        "the fail-closed outcome must be evidenced as a pricing block: {evaluation:#?}"
    );
}

#[test]
fn sized_acceptance_keeps_first_pass_size_when_repriced_resize_is_within_tolerance() {
    let mut strategy = ready_to_trade_strategy_with_live_fees(Decimal::ZERO, Decimal::ZERO);
    register_test_strategy_with_active_instruments(&mut strategy);
    strategy.config.order_notional_target = 10.0;
    strategy.config.maximum_position_notional = 10.0;
    strategy.config.risk_lambda = 0.5;
    strategy.config.sizing_ev_reference_bps = 500;
    strategy.config.vwap_depth_limit_bps = 5_000;
    strategy.config.book_impact_cap_bps = 5_000;
    strategy.config.edge_threshold_basis_points = 0;
    strategy.config.slippage_buffer_bps = 0;
    strategy
        .pricing
        .set_selected_pricing_spot(Some(fast_spot("bybit", 3_120.0, 1_200)));
    strategy
        .pricing
        .seed_ready_realized_vol(Some("<SOURCE_ID>".to_string()), 2.5, 1_200);

    let up_instrument_id = strategy
        .instrument_id_for_side(OutcomeSide::Up)
        .expect("UP instrument should be configured");
    let down_instrument_id = strategy
        .instrument_id_for_side(OutcomeSide::Down)
        .expect("DOWN instrument should be configured");
    assert!(strategy.active.books.update_from_deltas(&book_deltas(
        up_instrument_id,
        &[
            (BookAction::Clear, OrderSide::Buy, 0.49, 100.0),
            (BookAction::Add, OrderSide::Buy, 0.49, 100.0),
            (BookAction::Add, OrderSide::Sell, 0.50, 100.0),
        ],
    )));
    assert!(strategy.active.books.update_from_deltas(&book_deltas(
        down_instrument_id,
        &[
            (BookAction::Clear, OrderSide::Buy, 0.48, 100.0),
            (BookAction::Add, OrderSide::Buy, 0.48, 100.0),
            (BookAction::Add, OrderSide::Sell, 0.90, 100.0),
        ],
    )));

    let evaluation = strategy.entry_evaluation_at(1_200);

    assert!(
        evaluation.pricing_blocked_by.is_empty(),
        "fixed-point sized re-evaluation should remain executable: {evaluation:#?}"
    );
    assert_eq!(evaluation.selected_side, Some(OutcomeSide::Up));
    assert!(
        evaluation
            .sized_executable_edge
            .as_ref()
            .is_some_and(|edge| edge.trade_allowed),
        "accepted entry must include tradeable sized edge evidence: {evaluation:#?}"
    );
    let sized_notional = evaluation
        .sized_notional
        .expect("fixed-point acceptance should retain sized notional");
    let impact_cap_notional = evaluation
        .book_impact_cap_notional
        .expect("fixed-point acceptance should expose the impact cap");
    let preliminary_ev_per_notional = evaluation
        .up_worst_case_ev_bps
        .expect("selected side should expose preliminary EV")
        / BPS_DENOMINATOR;
    let preliminary_supported_notional = choose_robust_size(&RobustSizingInputs {
        expected_ev_per_notional: preliminary_ev_per_notional,
        ev_reference_per_notional: strategy.config.sizing_ev_reference_bps as f64 / BPS_DENOMINATOR,
        risk_lambda: strategy.config.risk_lambda,
        order_notional_target: strategy.config.order_notional_target,
        maximum_position_notional: strategy.config.maximum_position_notional,
        impact_cap_notional,
    });
    assert!(
        (sized_notional - preliminary_supported_notional).abs()
            <= notional_float_tolerance(preliminary_supported_notional),
        "accepted notional should keep the first-pass robust size when repricing converges: {evaluation:#?}"
    );
    let final_ev_per_notional = evaluation
        .expected_ev_per_notional
        .expect("accepted entry should expose final EV");
    let final_supported_notional = choose_robust_size(&RobustSizingInputs {
        expected_ev_per_notional: final_ev_per_notional,
        ev_reference_per_notional: strategy.config.sizing_ev_reference_bps as f64 / BPS_DENOMINATOR,
        risk_lambda: strategy.config.risk_lambda,
        order_notional_target: strategy.config.order_notional_target,
        maximum_position_notional: strategy.config.maximum_position_notional,
        impact_cap_notional,
    });
    assert!(
        (final_supported_notional - sized_notional).abs()
            <= notional_float_tolerance(sized_notional),
        "final re-priced EV should support the retained first-pass notional within tolerance: {evaluation:#?}"
    );
}

#[test]
fn executable_edge_fee_uses_exact_size_vwap_price_not_limit_price() {
    let mut strategy = test_strategy_with_fee_provider(Arc::new(PriceSensitiveEntryFeeProvider));
    configure_supported_market_quote_entry_order(&mut strategy);
    strategy.config.order_notional_target = 5.0;
    strategy.config.maximum_position_notional = 5.0;
    strategy.config.vwap_depth_limit_bps = 2_000;
    strategy.config.edge_threshold_basis_points = 0;
    strategy.config.slippage_buffer_bps = 0;
    strategy.config.warmup_tick_count = 2;
    strategy.apply_selection_snapshot(active_snapshot_with_start("MKT-1", 1_000));
    strategy.active.price_to_beat = Some(3_100.0);
    strategy.active.interval_open = Some(3_100.0);
    strategy.active.warmup_count = 2;
    strategy.active.last_reference_ts_ms = Some(1_200);
    strategy.active.fast_venue_incoherent = false;
    strategy
        .pricing
        .set_selected_pricing_spot(Some(fast_spot("bybit", 3_120.0, 1_200)));
    strategy
        .pricing
        .seed_ready_realized_vol(Some("<SOURCE_ID>".to_string()), 2.5, 1_200);
    strategy.pricing.last_lead_gap_probability = Some(probability(0.0));
    strategy.pricing.last_jitter_penalty_probability = Some(probability(0.0));
    register_test_strategy_with_active_instruments(&mut strategy);
    set_configured_books_depth(
        &mut strategy,
        &[
            (BookAction::Clear, OrderSide::Buy, 0.49, 100.0),
            (BookAction::Add, OrderSide::Buy, 0.49, 100.0),
            (BookAction::Add, OrderSide::Sell, 0.50, 5.0),
            (BookAction::Add, OrderSide::Sell, 0.60, 100.0),
        ],
    );

    let evaluation = strategy.entry_evaluation_at(1_200);

    assert_eq!(
        evaluation
            .up_executable_edge
            .as_ref()
            .map(|edge| edge.cost_breakdown.limit_price),
        Some(Some(0.60))
    );
    assert!(
        evaluation
            .up_executable_edge
            .as_ref()
            .and_then(|edge| executable_edge_fee_bps(Some(*edge)))
            .is_some_and(|fee_bps| (fee_bps - 5_000.0).abs() < 1e-9),
        "fee must be probed at exact-size VWAP price, not the last limit level: {evaluation:#?}"
    );
}

#[test]
fn executable_edge_fee_requires_cached_instrument_in_test_builds() {
    let mut strategy = test_strategy_with_fee_provider(Arc::new(PriceSensitiveEntryFeeProvider));
    configure_supported_market_quote_entry_order(&mut strategy);
    strategy.config.order_notional_target = 5.0;
    strategy.config.maximum_position_notional = 5.0;
    strategy.config.vwap_depth_limit_bps = 2_000;
    strategy.config.edge_threshold_basis_points = 0;
    strategy.config.slippage_buffer_bps = 0;
    strategy.config.warmup_tick_count = 2;
    strategy.apply_selection_snapshot(active_snapshot_with_start("MKT-1", 1_000));
    strategy.active.price_to_beat = Some(3_100.0);
    strategy.active.interval_open = Some(3_100.0);
    strategy.active.warmup_count = 2;
    strategy.active.last_reference_ts_ms = Some(1_200);
    strategy.active.fast_venue_incoherent = false;
    strategy
        .pricing
        .set_selected_pricing_spot(Some(fast_spot("bybit", 3_120.0, 1_200)));
    strategy
        .pricing
        .seed_ready_realized_vol(Some("<SOURCE_ID>".to_string()), 2.5, 1_200);
    strategy.pricing.last_lead_gap_probability = Some(probability(0.0));
    strategy.pricing.last_jitter_penalty_probability = Some(probability(0.0));
    set_configured_books_depth(
        &mut strategy,
        &[
            (BookAction::Clear, OrderSide::Buy, 0.49, 100.0),
            (BookAction::Add, OrderSide::Buy, 0.49, 100.0),
            (BookAction::Add, OrderSide::Sell, 0.50, 100.0),
        ],
    );

    let evaluation = strategy.entry_evaluation_at(1_200);

    assert_eq!(
        evaluation.pricing_blocked_by,
        vec![
            EntryPricingBlockReason::FeeUnavailable(OutcomeSide::Up),
            EntryPricingBlockReason::FeeUnavailable(OutcomeSide::Down),
        ],
        "test builds must not fall back to FeeProvider::fee_bps when the NT instrument is missing: {evaluation:#?}"
    );
    assert_eq!(evaluation.selected_side, None);
}

#[test]
fn executable_edge_blocks_unsupported_post_only_entry_shape() {
    let mut strategy = ready_to_trade_strategy_with_live_fees(Decimal::ZERO, Decimal::ZERO);
    strategy.config.entry_order.time_in_force = TimeInForce::Gtc;
    strategy.config.entry_order.is_post_only = true;
    strategy
        .pricing
        .set_selected_pricing_spot(Some(fast_spot("bybit", 3_120.0, 1_200)));
    strategy
        .pricing
        .seed_ready_realized_vol(Some("<SOURCE_ID>".to_string()), 2.5, 1_200);

    let evaluation = strategy.entry_evaluation_at(1_200);

    assert_eq!(
        evaluation.pricing_blocked_by,
        vec![
            EntryPricingBlockReason::ExecutableEdgeUnavailable(
                OutcomeSide::Up,
                BinaryOutcomeEdgeBlockReason::UnsupportedOrderShape
            ),
            EntryPricingBlockReason::ExecutableEdgeUnavailable(
                OutcomeSide::Down,
                BinaryOutcomeEdgeBlockReason::UnsupportedOrderShape
            ),
        ]
    );
    assert_eq!(evaluation.selected_side, None);
}

#[test]
fn entry_submission_blocks_legacy_limit_base_entry_shape_before_liability_sizing() {
    let mut strategy = ready_to_trade_strategy_with_live_fees(Decimal::ZERO, Decimal::ZERO);
    configure_limit_base_entry_order(&mut strategy);
    register_test_strategy_with_active_instruments(&mut strategy);
    strategy.config.order_notional_target = 5.0;
    strategy.config.maximum_position_notional = 5.0;
    strategy.config.risk_lambda = 0.0;
    strategy.config.book_impact_cap_bps = 1_000;
    strategy.config.vwap_depth_limit_bps = 2_000;
    strategy.config.slippage_buffer_bps = 0;
    strategy.config.edge_threshold_basis_points = 0;
    strategy
        .pricing
        .set_selected_pricing_spot(Some(fast_spot("bybit", 3_120.0, 1_200)));
    strategy
        .pricing
        .seed_ready_realized_vol(Some("<SOURCE_ID>".to_string()), 2.5, 1_200);
    set_configured_books_depth(
        &mut strategy,
        &[
            (BookAction::Clear, OrderSide::Buy, 0.49, 100.0),
            (BookAction::Add, OrderSide::Buy, 0.49, 100.0),
            (BookAction::Add, OrderSide::Sell, 0.50, 5.0),
            (BookAction::Add, OrderSide::Sell, 0.60, 100.0),
        ],
    );

    let decision = strategy.entry_submission_decision_at(1_200);

    assert_eq!(
        decision.blocked_reason,
        Some(ENTRY_BLOCK_REASON_ENTRY_PRICING_BLOCKED)
    );
    assert_eq!(
        decision.evaluation.pricing_blocked_by,
        vec![
            EntryPricingBlockReason::ExecutableEdgeUnavailable(
                OutcomeSide::Up,
                BinaryOutcomeEdgeBlockReason::UnsupportedOrderShape
            ),
            EntryPricingBlockReason::ExecutableEdgeUnavailable(
                OutcomeSide::Down,
                BinaryOutcomeEdgeBlockReason::UnsupportedOrderShape
            ),
        ],
        "legacy limit/base entry sizing must not bypass the Lane 1 supported-shape guard: {decision:#?}"
    );
    assert_eq!(decision.price, None);
    assert_eq!(decision.quantity_value, None);
}

#[test]
fn entry_submission_notional_guard_allows_scaled_float_noise() {
    let sized_notional = BPS_DENOMINATOR;
    let tolerance = notional_float_tolerance(sized_notional);
    let representational_overage = sized_notional + (tolerance / MIDPOINT_DIVISOR_F64);
    let material_overage = sized_notional + (tolerance * MIDPOINT_DIVISOR_F64);

    assert!(!limit_notional_exceeds_sized_notional(
        representational_overage,
        sized_notional
    ));
    assert!(limit_notional_exceeds_sized_notional(
        material_overage,
        sized_notional
    ));
}

#[test]
fn entry_submission_notional_guard_blocks_non_finite_inputs() {
    assert!(limit_notional_exceeds_sized_notional(
        f64::NAN,
        BPS_DENOMINATOR
    ));
    assert!(limit_notional_exceeds_sized_notional(
        BPS_DENOMINATOR,
        f64::INFINITY
    ));
}

#[test]
fn task6_entry_evaluation_uses_live_uncertainty_band_probability() {
    let mut strategy = ready_to_trade_strategy_with_live_fees(Decimal::ZERO, Decimal::ZERO);
    register_test_strategy_with_active_instruments(&mut strategy);
    strategy.config.order_notional_target = UNIT_F64;
    strategy.config.maximum_position_notional = UNIT_F64;
    strategy.config.edge_threshold_basis_points = 0;
    let cheap_outcome_price = UNIT_F64 / BPS_DENOMINATOR.sqrt();
    let book_quantity = BPS_DENOMINATOR;
    set_configured_books_depth(
        &mut strategy,
        &[
            (
                BookAction::Clear,
                OrderSide::Buy,
                cheap_outcome_price,
                book_quantity,
            ),
            (
                BookAction::Add,
                OrderSide::Buy,
                cheap_outcome_price,
                book_quantity,
            ),
            (
                BookAction::Add,
                OrderSide::Sell,
                cheap_outcome_price,
                book_quantity,
            ),
        ],
    );
    strategy
        .pricing
        .set_selected_pricing_spot(Some(fast_spot("bybit", 3_100.4, 1_200)));
    strategy
        .pricing
        .seed_ready_realized_vol(Some("<SOURCE_ID>".to_string()), 2.5, 1_200);
    strategy.pricing.last_lead_gap_probability = Some(probability(0.02));
    strategy.pricing.last_jitter_penalty_probability = Some(probability(0.01));

    let decision = strategy.entry_evaluation_at(1_200);

    assert!(decision.pricing_blocked_by.is_empty());
    assert!(
        decision
            .uncertainty_band_probability
            .is_some_and(|value| value.value() > 0.0)
    );
}

#[test]
fn task6_entry_evaluation_requires_live_uncertainty_components() {
    let mut strategy =
        ready_to_trade_strategy_with_live_fees(Decimal::new(250, 2), Decimal::new(250, 2));
    register_test_strategy_with_active_instruments(&mut strategy);
    strategy
        .pricing
        .set_selected_pricing_spot(Some(fast_spot("bybit", 3_100.4, 1_200)));
    strategy
        .pricing
        .seed_ready_realized_vol(Some("<SOURCE_ID>".to_string()), 2.5, 1_200);
    strategy.pricing.last_lead_gap_probability = None;
    strategy.pricing.last_jitter_penalty_probability = None;

    let decision = strategy.entry_evaluation_at(1_200);

    assert_eq!(
        decision.pricing_blocked_by,
        vec![EntryPricingBlockReason::UncertaintyBandUnavailable]
    );
    assert_eq!(decision.uncertainty_band_probability, None);
}

#[test]
fn task6_entry_evaluation_applies_theta_scaled_threshold_at_boundary() {
    let mut strategy = ready_to_trade_strategy_with_live_fees(Decimal::ZERO, Decimal::ZERO);
    register_test_strategy_with_active_instruments(&mut strategy);
    strategy
        .pricing
        .set_selected_pricing_spot(Some(fast_spot("bybit", 3_120.0, 1_200)));
    strategy
        .pricing
        .seed_ready_realized_vol(Some("<SOURCE_ID>".to_string()), 2.5, 1_200);
    strategy.config.edge_threshold_basis_points = 2_000;

    let baseline = strategy.entry_evaluation_at(1_200);
    assert_eq!(baseline.selected_side, Some(OutcomeSide::Up));

    strategy.config.theta_decay_factor = 100.0;
    strategy.active.last_reference_ts_ms = Some(291_000);
    strategy
        .pricing
        .set_selected_pricing_spot(Some(fast_spot("bybit", 3_120.0, 291_000)));
    strategy
        .pricing
        .seed_ready_realized_vol(Some("<SOURCE_ID>".to_string()), 2.5, 291_000);
    let near_expiry = strategy.entry_evaluation_at(291_000);

    assert!(near_expiry.gate.blocked_by.is_empty());
    assert!(
        near_expiry.pricing_blocked_by.contains(
            &EntryPricingBlockReason::ExecutableEdgeUnavailable(
                OutcomeSide::Up,
                BinaryOutcomeEdgeBlockReason::EdgeBelowThreshold
            )
        ),
        "theta-scaled threshold miss should surface as an edge-below-threshold pricing block: {near_expiry:#?}"
    );
    assert!(near_expiry.up_worst_case_ev_bps.is_some());
    assert!(near_expiry.min_worst_case_ev_bps.is_some());
    assert_eq!(near_expiry.selected_side, None);
    assert!(
        near_expiry
            .min_worst_case_ev_bps
            .zip(near_expiry.up_worst_case_ev_bps)
            .is_some_and(|(threshold, up_ev)| threshold >= up_ev),
        "theta-scaled threshold should close the entry boundary near expiry"
    );
}

#[test]
fn entry_evaluation_log_fields_capture_parameters_and_omissions() {
    let mut strategy = ready_to_trade_strategy_with_live_fees(Decimal::ZERO, Decimal::ZERO);
    register_test_strategy_with_active_instruments(&mut strategy);
    strategy.observe_reference_snapshot(
        &ReferenceSnapshot {
            ts_ms: 1_200,
            topic: "platform.reference.test.spot".to_string(),
            fair_value: Some(3_100.5),
            confidence: 1.0,
            venues: vec![
                oracle_venue("reference", 1.0, 3_100.5, 1_200),
                orderbook_venue("bybit", 0.9, 3_101.0, 1_200),
            ],
        },
        LocalReceiveMs::new(1_200),
    );
    strategy
        .pricing
        .seed_ready_realized_vol(Some("<SOURCE_ID>".to_string()), 2.5, 1_200);

    let evaluation = strategy.entry_evaluation_at(1_200);
    let submission = strategy.entry_submission_decision_at(1_200);
    let fields = strategy.entry_evaluation_log_fields_at(1_200, &submission);

    assert_eq!(fields.market_id.as_deref(), Some("MKT-1"));
    assert_eq!(fields.phase, SelectionPhase::Active);
    assert_eq!(fields.spot_venue_name.as_deref(), Some("bybit"));
    assert_eq!(fields.spot_price, Some(3_101.0));
    assert_eq!(fields.reference_current_price, Some(3_100.5));
    assert_eq!(fields.interval_open, Some(3_100.0));
    assert_eq!(fields.realized_vol, Some(2.5));
    assert_eq!(
        fields.realized_vol_source_venue.as_deref(),
        Some("<SOURCE_ID>")
    );
    assert_eq!(fields.realized_vol_source_ts_ms, Some(1_200));
    assert_eq!(
        fields.fair_probability_up,
        evaluation.fair_probability_up.map(Probability::value)
    );
    assert_eq!(fields.selected_side, evaluation.selected_side);
    assert!(fields.uncertainty_band_probability.is_some());
    assert!(fields.uncertainty_band_live);
    assert_eq!(
        fields.uncertainty_band_reason,
        "derived_from_lead_gap_jitter_time_and_fee"
    );
    assert!(fields.up_entry_limit_price.is_some());
    assert!(fields.down_entry_limit_price.is_some());
    assert!(fields.up_gross_cost_cents.is_some());
    assert!(fields.down_gross_cost_cents.is_some());
    assert!(fields.up_fee_cost_cents.is_some());
    assert!(fields.down_fee_cost_cents.is_some());
    assert!(fields.up_slippage_buffer_cents.is_some());
    assert!(fields.down_slippage_buffer_cents.is_some());
    assert!(fields.up_total_adjusted_cost_cents.is_some());
    assert!(fields.down_total_adjusted_cost_cents.is_some());
    assert!(fields.up_edge_cents_per_share.is_some());
    assert!(fields.down_edge_cents_per_share.is_some());
    assert!(fields.lead_quality_policy_applied);
    assert!(
        fields
            .expected_ev_per_notional
            .is_some_and(|value| value > 0.0)
    );
    assert_eq!(
        fields.maximum_position_notional,
        strategy.config.maximum_position_notional
    );
    assert_eq!(fields.risk_lambda, strategy.config.risk_lambda);
    assert_eq!(
        fields.order_notional_target,
        strategy.config.order_notional_target
    );
    assert_eq!(
        fields.sizing_ev_reference_bps,
        strategy.config.sizing_ev_reference_bps
    );
    let rendered_fields = format!("{fields:?}");
    assert!(
        rendered_fields.contains("order_notional_target"),
        "entry-evaluation log fields must expose target dollars: {rendered_fields}"
    );
    assert!(
        rendered_fields.contains("sizing_ev_reference_bps"),
        "entry-evaluation log fields must expose the EV sizing reference: {rendered_fields}"
    );
    assert_eq!(
        fields.book_impact_cap_bps,
        strategy.config.book_impact_cap_bps
    );
    assert!(
        fields
            .book_impact_cap_notional
            .is_some_and(|value| value > 0.0)
    );
    assert!(fields.sized_notional.is_some_and(|value| value > 0.0));
    assert!(!fields.final_fee_amount_known);
}

#[test]
fn exit_hold_ev_does_not_require_uncertainty_band_components() {
    let mut strategy = ready_to_trade_strategy_with_live_fees(Decimal::ZERO, Decimal::ZERO);
    let open_position = OpenPositionState {
        lifecycle: BoltV3PositionMarketLifecycle::from_entry_context(
            Some("MKT-1".to_string()),
            Some(OutcomeSide::Up),
            Some(3_100.0),
            Some(3_100.0),
            Some(301_000),
            Some(1_000),
            Some(300),
        ),
        instrument_id: strategy.active.books.up.instrument_id.unwrap(),
        position_id: PositionId::from("P-UP-MISSING-UNCERTAINTY"),
        outcome_fees: strategy.active.outcome_fees.clone(),
        historical_entry_fee_bps: Some(0.0),
        entry_order_side: OrderSide::Buy,
        side: PositionSide::Long,
        quantity: Quantity::new(10.0, 2),
        avg_px_open: 0.450,
        book: strategy.active.books.up.clone(),
    };
    set_managed_position(
        &mut strategy,
        open_position,
        ManagedPositionOrigin::StrategyEntry,
    );
    strategy
        .pricing
        .set_selected_pricing_spot(Some(fast_spot("bybit", 3_099.5, 1_200)));
    strategy
        .pricing
        .seed_ready_realized_vol(Some("<SOURCE_ID>".to_string()), 2.5, 1_200);
    strategy.pricing.last_lead_gap_probability = None;
    strategy.pricing.last_jitter_penalty_probability = None;

    let decision = strategy.exit_submission_decision_at(1_200);

    assert!(decision.evaluation.hold_ev_bps.is_some());
    assert!(decision.evaluation.exit_ev_bps.is_some());
}

#[test]
fn exit_hold_ev_uses_raw_fair_probability_symmetrically_with_exit_ev() {
    let mut strategy = ready_to_trade_strategy_with_live_fees(Decimal::ZERO, Decimal::ZERO);
    let open_position = OpenPositionState {
        lifecycle: BoltV3PositionMarketLifecycle::from_entry_context(
            Some("MKT-1".to_string()),
            Some(OutcomeSide::Up),
            Some(3_100.0),
            Some(3_100.0),
            Some(301_000),
            Some(1_000),
            Some(300),
        ),
        instrument_id: strategy.active.books.up.instrument_id.unwrap(),
        position_id: PositionId::from("P-UP-SYMMETRIC-HOLD"),
        outcome_fees: strategy.active.outcome_fees.clone(),
        historical_entry_fee_bps: Some(0.0),
        entry_order_side: OrderSide::Buy,
        side: PositionSide::Long,
        quantity: Quantity::new(10.0, 2),
        avg_px_open: 0.450,
        book: strategy.active.books.up.clone(),
    };
    set_managed_position(
        &mut strategy,
        open_position,
        ManagedPositionOrigin::StrategyEntry,
    );
    strategy
        .pricing
        .set_selected_pricing_spot(Some(fast_spot("bybit", 3_101.0, 1_200)));
    strategy
        .pricing
        .seed_ready_realized_vol(Some("<SOURCE_ID>".to_string()), 2.5, 1_200);

    let fair_probability_up = strategy
        .current_position_fair_probability_up_at(1_200)
        .expect("ready position should produce fair probability")
        .value();
    let hold_ev_bps = strategy
        .current_hold_ev_bps_at(1_200, OutcomeSide::Up)
        .expect("ready position should produce hold EV");
    let expected_hold_ev_bps = ((fair_probability_up - 0.450) / 0.450) * BPS_DENOMINATOR;

    assert!(
        (hold_ev_bps - expected_hold_ev_bps).abs() < 1e-9,
        "hold EV must use the same raw fair value basis as exit EV; hold={hold_ev_bps} expected={expected_hold_ev_bps}"
    );
}

#[test]
fn position_probability_and_hold_ev_accept_ready_surfaced_zero_realized_volatility() {
    let mut strategy = ready_to_trade_strategy_with_live_fees(Decimal::ZERO, Decimal::ZERO);
    let open_position = OpenPositionState {
        lifecycle: BoltV3PositionMarketLifecycle::from_entry_context(
            Some("MKT-1".to_string()),
            Some(OutcomeSide::Up),
            Some(3_100.0),
            Some(3_100.0),
            Some(301_000),
            Some(1_000),
            Some(300),
        ),
        instrument_id: strategy.active.books.up.instrument_id.unwrap(),
        position_id: PositionId::from("P-UP-ZERO-RV"),
        outcome_fees: strategy.active.outcome_fees.clone(),
        historical_entry_fee_bps: Some(0.0),
        entry_order_side: OrderSide::Buy,
        side: PositionSide::Long,
        quantity: Quantity::new(10.0, 2),
        avg_px_open: 0.450,
        book: strategy.active.books.up.clone(),
    };
    set_managed_position(
        &mut strategy,
        open_position,
        ManagedPositionOrigin::StrategyEntry,
    );
    strategy.pricing.observe_realized_vol_snapshot(
        crate::bolt_v3_realized_volatility::RealizedVolSnapshot {
            surface_id: "<surface_id>".to_string(),
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
            sources_used: vec!["<SOURCE_ID_A>".to_string()],
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

    assert_eq!(
        strategy.current_position_fair_probability_up_at(1_200),
        Some(probability(1.0))
    );
    assert!(
        strategy
            .exit_evaluation_at(1_200)
            .hold_ev_bps
            .is_some_and(f64::is_finite)
    );
}
