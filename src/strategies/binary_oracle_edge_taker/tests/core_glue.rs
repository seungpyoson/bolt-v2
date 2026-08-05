#![cfg(test)]

use super::*;

#[test]
fn decision_evidence_failure_rejects_before_nt_submit() {
    let instrument_id = selected_entry_instrument(&ready_to_trade_strategy());
    // Seed a (zero) fee for the order's instrument so admission clears the
    // fee-bound guard and the FailingDecisionEvidenceWriter is what rejects
    // the order before NT submit — the behavior this test pins.
    let mut strategy = test_strategy_with_fee_provider_and_decision_evidence(
        RecordingFeeProvider::with_fee(&instrument_id.to_string(), Decimal::ZERO),
        failing_decision_evidence(),
    );
    register_test_strategy_with_instrument(&mut strategy, &instrument_id);
    let quantity = Quantity::new(1.0, 2);
    let price = Price::new(0.50, 2);
    let client_order_id = ClientOrderId::from("O-19700101-000000-001-001-1");
    let order = nautilus_model::orders::OrderAny::Limit(
        nautilus_model::orders::LimitOrder::new_checked(
            nautilus_model::identifiers::TraderId::from("TRADER-001"),
            StrategyId::from(strategy.config.strategy_id.as_str()),
            instrument_id,
            client_order_id,
            OrderSide::Buy,
            quantity,
            price,
            TimeInForce::Fok,
            None,
            false,
            false,
            false,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            nautilus_core::UUID4::new(),
            nautilus_core::UnixNanos::from(1_u64),
        )
        .expect("limit order should be valid"),
    );
    let intent = crate::bolt_v3_order_execution::order_intent_details_from_compiled_order(
        strategy.config.strategy_id.clone(),
        price.to_string(),
        &order,
    );

    let error = strategy
        .submit_order_with_decision_evidence(
            intent,
            BoltV3SubmitIntentKind::Entry,
            order,
            BoltV3SubmitContext::with_client_id(ClientId::from("POLYMARKET")),
        )
        .expect_err("evidence failure must reject before NT submit");

    assert!(
        error
            .to_string()
            .contains("evidence commit indeterminate during write"),
        "{error:#}"
    );
}

#[test]
fn effective_stale_bound_uses_gate_freshness_as_single_source_when_armed() {
    // A5: the gate-approved reference-quote freshness bound is the single
    // authoritative source for the armed live path, plumbed into the
    // forced-flat stale check as the STRICTER of (gate bound, strategy
    // config bound) so arming can only tighten, never loosen.
    let submit_admission = Arc::new(
        crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new(
            recording_decision_evidence(),
        ),
    );
    let mut strategy = test_strategy_with_fee_provider_decision_evidence_and_submit_admission(
        RecordingFeeProvider::cold(),
        recording_decision_evidence(),
        submit_admission.clone(),
    );

    strategy.config.forced_flat_stale_reference_ms = 1_500;
    assert_eq!(strategy.effective_stale_reference_after_ms(), 1_500);

    strategy.config.forced_flat_stale_reference_ms = 20_000;
    assert_eq!(
        strategy.effective_stale_reference_after_ms(),
        20_000,
        "strategy config is the stale-reference freshness bound"
    );

    strategy.config.forced_flat_stale_reference_ms = 1_500;
    assert_eq!(
        strategy.effective_stale_reference_after_ms(),
        1_500,
        "strategy config updates must apply directly"
    );
}

#[test]
fn limit_if_touched_rejects_nt_side_price_invariants_before_factory() {
    let mut strategy = ready_to_trade_strategy_with_live_fees(Decimal::ZERO, Decimal::ZERO);
    let _cache = register_test_strategy(&mut strategy);
    let instrument_id = selected_entry_instrument(&strategy);
    let quantity = Quantity::new(2.0, 2);

    strategy.config.entry_order.order_type = OrderType::LimitIfTouched;
    strategy.config.entry_order.time_in_force = TimeInForce::Gtc;
    strategy.config.entry_order.trigger_price = Some(0.41);
    let buy_error = strategy
        .build_configured_entry_order(
            instrument_id,
            OrderSide::Buy,
            quantity,
            Price::new(0.40, 2),
            ClientOrderId::from("O-19700101-000000-001-011-1"),
        )
        .expect_err("BUY LimitIfTouched with trigger above limit should fail before NT factory");
    assert!(
        buy_error.to_string().contains("trigger_price") && buy_error.to_string().contains("<="),
        "{buy_error}"
    );

    strategy.config.exit_order.order_type = OrderType::LimitIfTouched;
    strategy.config.exit_order.time_in_force = TimeInForce::Gtc;
    strategy.config.exit_order.trigger_price = Some(0.44);
    let sell_error = strategy
        .build_configured_exit_order(
            instrument_id,
            OrderSide::Sell,
            quantity,
            Price::new(0.45, 2),
            ClientOrderId::from("O-19700101-000000-001-012-1"),
        )
        .expect_err("SELL LimitIfTouched with trigger below limit should fail before NT factory");
    assert!(
        sell_error.to_string().contains("trigger_price") && sell_error.to_string().contains(">="),
        "{sell_error}"
    );
}

#[test]
fn production_strategy_has_no_offline_readiness_seed_arming() {
    // #551 regression lock: the offline operator readiness seed must never
    // again arm `price_to_beat`, the reference quote, or the realized-vol
    // bootstrap. Once #553 wired the live Chainlink strike, the live strike
    // (`observe_resolution_strike`) and live quotes are the ONLY sources.
    // `production_module_source_text` returns the strategy directory's
    // production halves only (each file's `#[cfg(test)] mod tests` excluded),
    // so these needle literals (which live in this test) cannot match the
    // production source they guard.
    let production = crate::bolt_v3_source_integrity::production_module_source_text(
        crate::bolt_v3_source_integrity::STRATEGY_KEY,
    );
    for forbidden in [
        "apply_source_owned_readiness_seed",
        "runtime_readiness_seed",
        "BoltV3RuntimeReadinessSeed",
    ] {
        assert!(
            !production.contains(forbidden),
            "offline readiness-seed symbol `{forbidden}` reappeared in production strategy \
             code; #551 removed it — price_to_beat must come only from the live Chainlink strike"
        );
    }
}

#[test]
fn book_delta_submit_admission_error_does_not_escape_actor_loop() {
    let rejecting_submit_admission =
        submit_admission_with_provider_cap(Decimal::new(1, 2), recording_decision_evidence());
    let mut direct = ready_to_trade_strategy_with_decision_evidence_and_submit_admission(
        recording_decision_evidence(),
        rejecting_submit_admission.clone(),
    );
    register_test_strategy_with_active_instruments(&mut direct);
    configure_supported_market_quote_entry_order(&mut direct);
    direct.config.order_notional_target = 25.0;
    direct.config.maximum_position_notional = 25.0;
    direct.config.risk_lambda = 0.0001;
    let direct_error = direct
        .try_submit_entry_order(1_200)
        .expect_err("test setup must reach submit-admission cap rejection");
    assert!(
        direct_error
            .to_string()
            .contains("notional cap is exceeded"),
        "test setup must prove submit-admission failure path: {direct_error:#}"
    );

    let rejecting_submit_admission =
        submit_admission_with_provider_cap(Decimal::new(1, 2), recording_decision_evidence());
    let mut strategy = ready_to_trade_strategy_with_decision_evidence_and_submit_admission(
        recording_decision_evidence(),
        rejecting_submit_admission,
    );
    register_test_strategy_with_active_instruments(&mut strategy);
    configure_supported_market_quote_entry_order(&mut strategy);
    strategy.config.order_notional_target = 25.0;
    strategy.config.maximum_position_notional = 25.0;
    strategy.config.risk_lambda = 0.0001;
    let instrument_id = selected_entry_instrument(&strategy);
    let decision = strategy.entry_submission_decision_at(1_200);
    assert!(
        decision.instrument_id.is_some()
            && decision.order_side.is_some()
            && decision.price.is_some()
            && decision.quantity_value.is_some()
            && decision.blocked_reason.is_none(),
        "test setup must reach submit admission path; got {decision:#?}"
    );

    let result = strategy.on_book_deltas(&book_deltas(
        instrument_id,
        &[(BookAction::Update, OrderSide::Sell, 0.44, 500.0)],
    ));

    assert!(
        result.is_ok(),
        "book-delta submit failures must be logged and contained inside the strategy actor: {result:#?}"
    );
    assert!(matches!(strategy.exposure, ExposureState::Flat));
}

#[test]
fn book_delta_exit_submit_admission_error_does_not_escape_actor_loop() {
    let rejecting_submit_admission =
        submit_admission_with_provider_cap(Decimal::new(1, 0), recording_decision_evidence());
    let mut strategy = ready_to_trade_strategy_with_decision_evidence_and_submit_admission(
        recording_decision_evidence(),
        rejecting_submit_admission.clone(),
    );
    strategy.active.phase = SelectionPhase::Freeze;
    let instrument_id = selected_entry_instrument(&strategy);
    let position = materialize_configured_position(
        &mut strategy,
        instrument_id,
        PositionId::from("P-EXIT-SUBMIT-ERROR"),
        Quantity::new(10.0, 2),
        0.450,
    );
    set_managed_position(
        &mut strategy,
        position,
        ManagedPositionOrigin::StrategyEntry,
    );
    register_test_strategy_with_active_instruments(&mut strategy);
    let decision = strategy.exit_submission_decision_at(1_200);
    assert!(
        decision.instrument_id.is_some()
            && decision.order_side.is_some()
            && decision.price.is_some()
            && decision.quantity.is_some()
            && decision.blocked_reason.is_none(),
        "test setup must reach exit submit admission path; got {decision:#?}"
    );
    let managed_position = strategy
        .managed_position()
        .expect("managed position should remain available for exit admission setup");
    let exit_order_side = decision
        .order_side
        .expect("exit decision should include order side");
    let exit_quantity = Decimal::from_f64(
        decision
            .quantity
            .expect("exit decision should include quantity")
            .as_f64(),
    )
    .expect("exit quantity should convert to decimal");
    let position_quantity = Decimal::from_f64(managed_position.position.quantity.as_f64())
        .expect("position quantity should convert to decimal");
    rejecting_submit_admission
        .admit(&BoltV3SubmitAdmissionRequest {
            strategy_id: strategy.config.strategy_id.clone(),
            execution_client_id: strategy.config.client_id.clone(),
            client_order_id: "EXIT-SLOT-ALREADY-USED".to_string(),
            instrument_id: managed_position.position.instrument_id.to_string(),
            notional: Decimal::new(1, 0),
            order_side: exit_order_side,
            order_quantity: exit_quantity,
            intent_kind: BoltV3SubmitIntentKind::RiskReducingExit,
            risk_reducing_exit_proof: Some(BoltV3RiskReducingExitProof {
                position_id: managed_position.position.position_id.to_string(),
                instrument_id: managed_position.position.instrument_id.to_string(),
                position_side: managed_position.position.side,
                exit_order_side,
                position_quantity,
                exit_quantity,
            }),
            kill_switch_forced_reduction: None,
            admission_evidence: None,
        })
        .expect("test setup should consume the only risk-reducing exit slot")
        .commit_submitted();

    let result = strategy.on_book_deltas(&book_deltas(
        instrument_id,
        &[(BookAction::Update, OrderSide::Buy, 0.44, 500.0)],
    ));

    assert!(
        result.is_ok(),
        "book-delta exit submit failures must be logged and contained inside the strategy actor: {result:#?}"
    );
    assert!(matches!(strategy.exposure, ExposureState::Managed(_)));
    assert_eq!(strategy.last_reported_exposure_occupancy.get(), None);
}

#[test]
fn fees_ready_requires_both_outcome_tokens_before_refresh_can_succeed() {
    let fee_provider = RecordingFeeProvider::cold();
    let mut strategy = test_strategy_with_fee_provider(fee_provider.clone());
    strategy.apply_selection_snapshot(active_snapshot("MKT-1"));

    assert!(!strategy.active.outcome_fees.up_ready);
    assert!(!strategy.active.outcome_fees.down_ready);

    fee_provider.set_fee("condition-MKT-1-MKT-1-UP.POLYMARKET", Decimal::new(175, 2));
    strategy.refresh_fee_readiness();
    assert!(strategy.active.outcome_fees.up_ready);
    assert!(!strategy.active.outcome_fees.down_ready);

    fee_provider.set_fee(
        "condition-MKT-1-MKT-1-DOWN.POLYMARKET",
        Decimal::new(180, 2),
    );
    strategy.refresh_fee_readiness();

    assert!(strategy.active.outcome_fees.up_ready);
    assert!(strategy.active.outcome_fees.down_ready);
}

#[tokio::test(flavor = "current_thread")]
async fn market_activation_and_switch_warm_both_outcome_fee_tokens() {
    let fee_provider = RecordingFeeProvider::cold();
    let mut strategy = test_strategy_with_fee_provider(fee_provider.clone());

    strategy.apply_selection_snapshot(active_snapshot("MKT-1"));
    tokio::task::yield_now().await;
    strategy.apply_selection_snapshot(active_snapshot("MKT-2"));
    tokio::task::yield_now().await;

    assert_eq!(
        fee_provider.warm_calls(),
        vec![
            "condition-MKT-1-MKT-1-UP.POLYMARKET".to_string(),
            "condition-MKT-1-MKT-1-DOWN.POLYMARKET".to_string(),
            "condition-MKT-2-MKT-2-UP.POLYMARKET".to_string(),
            "condition-MKT-2-MKT-2-DOWN.POLYMARKET".to_string(),
        ]
    );
}

#[test]
fn fee_readiness_stays_false_until_both_outcome_fees_are_available() {
    let fee_provider = RecordingFeeProvider::cold();
    let mut strategy = test_strategy_with_fee_provider(fee_provider.clone());
    strategy.apply_selection_snapshot(active_snapshot("MKT-1"));

    assert!(!strategy.active.outcome_fees.up_ready);
    assert!(!strategy.active.outcome_fees.down_ready);

    fee_provider.set_fee("condition-MKT-1-MKT-1-UP.POLYMARKET", Decimal::new(175, 2));
    strategy.refresh_fee_readiness();
    assert!(strategy.active.outcome_fees.up_ready);
    assert!(!strategy.active.outcome_fees.down_ready);

    fee_provider.set_fee(
        "condition-MKT-1-MKT-1-DOWN.POLYMARKET",
        Decimal::new(180, 2),
    );
    strategy.refresh_fee_readiness();
    assert!(strategy.active.outcome_fees.up_ready);
    assert!(strategy.active.outcome_fees.down_ready);
}

#[test]
fn book_delta_refreshes_fee_readiness_after_warm_populates_provider() {
    let fee_provider = RecordingFeeProvider::cold();
    let mut strategy = ready_to_trade_strategy();
    strategy.context = StrategyBuildContext::new(
        fee_provider.clone(),
        recording_decision_evidence(),
        Arc::new(
            crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new(
                recording_decision_evidence(),
            ),
        ),
        crate::bolt_v3_order_execution::BoltV3OrderExecutionPolicy::live(),
        fixture_execution_venue(),
    );
    strategy.active.outcome_fees.up_ready = false;
    strategy.active.outcome_fees.down_ready = false;
    register_test_strategy_with_active_instruments(&mut strategy);

    let up_instrument_id = strategy
        .active
        .outcome_fees
        .up_instrument_id
        .expect("test active market should have up outcome");
    let down_instrument_id = strategy
        .active
        .outcome_fees
        .down_instrument_id
        .expect("test active market should have down outcome");
    fee_provider.set_fee(up_instrument_id.to_string().as_str(), Decimal::new(100, 2));
    fee_provider.set_fee(
        down_instrument_id.to_string().as_str(),
        Decimal::new(100, 2),
    );

    strategy
        .on_book_deltas(&book_deltas(
            up_instrument_id,
            &[(BookAction::Update, OrderSide::Sell, 0.45, 500.0)],
        ))
        .expect("book delta should not escape actor loop");

    assert!(strategy.active.outcome_fees.market_ready());
}

#[test]
fn switch_resets_fee_readiness_fail_closed_even_if_provider_has_cached_fee() {
    let fee_provider = RecordingFeeProvider::cold();
    fee_provider.set_fee("condition-MKT-1-MKT-1-UP.POLYMARKET", Decimal::new(175, 2));
    fee_provider.set_fee(
        "condition-MKT-1-MKT-1-DOWN.POLYMARKET",
        Decimal::new(180, 2),
    );
    let mut strategy = test_strategy_with_fee_provider(fee_provider);
    {
        let active = &mut strategy.active;
        active.outcome_fees.up_ready = true;
        active.outcome_fees.down_ready = true;
    }

    strategy.apply_selection_snapshot(active_snapshot("MKT-2"));

    assert!(!strategy.active.outcome_fees.up_ready);
    assert!(!strategy.active.outcome_fees.down_ready);
}

#[test]
fn switch_with_cached_fee_rates_stays_ready_while_refresh_runs() {
    let fee_provider = RecordingFeeProvider::cold();
    fee_provider.set_fee("condition-MKT-2-MKT-2-UP.POLYMARKET", Decimal::new(175, 2));
    fee_provider.set_fee(
        "condition-MKT-2-MKT-2-DOWN.POLYMARKET",
        Decimal::new(180, 2),
    );
    let mut strategy = test_strategy_with_fee_provider(fee_provider);

    strategy.apply_selection_snapshot(active_snapshot("MKT-1"));
    strategy.apply_selection_snapshot(active_snapshot("MKT-2"));

    assert!(strategy.active.outcome_fees.up_ready);
    assert!(strategy.active.outcome_fees.down_ready);
}

#[test]
fn same_market_new_interval_with_cached_fee_rates_stays_ready_while_refresh_runs() {
    let fee_provider = RecordingFeeProvider::cold();
    fee_provider.set_fee("condition-MKT-1-MKT-1-UP.POLYMARKET", Decimal::new(175, 2));
    fee_provider.set_fee(
        "condition-MKT-1-MKT-1-DOWN.POLYMARKET",
        Decimal::new(180, 2),
    );
    let mut strategy = test_strategy_with_fee_provider(fee_provider);

    strategy.apply_selection_snapshot(active_snapshot_with_start("MKT-1", 1_000));
    assert!(strategy.active.outcome_fees.market_ready());

    strategy.apply_selection_snapshot(active_snapshot_with_start("MKT-1", 2_000));

    assert!(strategy.active.outcome_fees.up_ready);
    assert!(strategy.active.outcome_fees.down_ready);
}

#[test]
fn task5_missing_reference_timestamp_is_stale_reference() {
    // A14: a never-observed reference (None timestamp) is the maximally
    // stale condition and must classify as StaleReference, not as fresh.
    let reasons = evaluate_forced_flat_predicates(&ForcedFlatInputs {
        frozen: false,
        metadata_matches_selection: true,
        last_reference_ts_ms: None,
        now_ms: 1_250,
        stale_reference_after_ms: 1_500,
        liquidity_available: Some(500.0),
        min_liquidity_required: 100.0,
        fast_venue_incoherent: false,
    });

    assert_eq!(reasons, vec![ForcedFlatReason::StaleReference]);
}
