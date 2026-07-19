#![cfg(test)]

use super::*;
use rust_decimal::prelude::ToPrimitive;

#[test]
fn ungated_submit_admission_allows_after_evidence_before_nt_submit() {
    let submit_admission = Arc::new(
        crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new(Arc::new(
            RecordingDecisionEvidenceWriter,
        )),
    );
    let instrument_id = selected_entry_instrument(&ready_to_trade_strategy());
    // The canonical quote leaves the reservation within the configured cap; with no optional gate armed,
    // production admission now allows the submit to reach NT.
    let mut strategy = test_strategy_with_economics_source_decision_evidence_and_submit_admission(
        RecordingEconomicsAdmissionSource::cold(),
        Arc::new(RecordingDecisionEvidenceWriter),
        submit_admission.clone(),
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
    let intent = crate::bolt_v3_decision_evidence::BoltV3OrderIntentEvidence::from_compiled_order(
        strategy.config.strategy_id.clone(),
        crate::bolt_v3_decision_evidence::BoltV3OrderIntentKind::Entry,
        price.to_string(),
        &order,
    );

    strategy
        .submit_order_with_decision_evidence(
            intent,
            order,
            BoltV3SubmitContext::with_client_id(ClientId::from("POLYMARKET")),
            test_gross_expected_value(),
        )
        .expect("ungated admission should reach NT submit");
    assert_eq!(
        submit_admission.admitted_order_count(),
        1,
        "ungated admission should consume live submit capacity"
    );
}

#[test]
fn shadow_policy_records_evidence_and_admission_without_nt_submit() {
    let evidence = Arc::new(RecordingSequencedDecisionEvidenceWriter::default());
    let submit_admission = Arc::new(
        crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new(evidence.clone()),
    );
    let instrument_id = selected_entry_instrument(&ready_to_trade_strategy());
    let mut strategy = test_strategy_with_economics_source_decision_evidence_and_submit_admission(
        RecordingEconomicsAdmissionSource::cold(),
        evidence.clone(),
        submit_admission.clone(),
    );
    set_shadow_order_execution_policy(&mut strategy);
    register_test_strategy_with_instrument(&mut strategy, &instrument_id);
    let (risk_handler, risk_messages) =
        get_typed_into_message_saving_handler::<TradingCommand>(None);
    msgbus::register_trading_command_endpoint(
        MessagingSwitchboard::risk_engine_queue_execute(),
        risk_handler,
    );
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
    let intent = crate::bolt_v3_decision_evidence::BoltV3OrderIntentEvidence::from_compiled_order(
        strategy.config.strategy_id.clone(),
        crate::bolt_v3_decision_evidence::BoltV3OrderIntentKind::Entry,
        price.to_string(),
        &order,
    );

    strategy
        .submit_order_with_decision_evidence(
            intent,
            order,
            BoltV3SubmitContext::with_client_id(ClientId::from("POLYMARKET")),
            test_gross_expected_value(),
        )
        .expect("shadow submission should still pass evidence and admission");
    assert_eq!(
        submit_admission.admitted_order_count(),
        0,
        "shadow policy records observed admission without consuming live capacity"
    );
    assert!(
        risk_messages.get_messages().is_empty(),
        "shadow policy must not emit an NT SubmitOrder command"
    );
    let events = evidence.events();
    assert!(
        events
            .iter()
            .any(|event| matches!(event, RecordedDecisionEvidenceEvent::OrderIntent(_))),
        "shadow submission must still record order-intent evidence"
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, RecordedDecisionEvidenceEvent::AdmissionDecision(_))),
        "shadow submission must still record admission evidence"
    );
}

#[test]
fn provider_limited_submit_admission_allows_nt_submit_after_evidence() {
    let submit_admission = submit_admission_with_provider_cap(
        Decimal::new(1, 0),
        Arc::new(RecordingDecisionEvidenceWriter),
    );
    let instrument_id = selected_entry_instrument(&ready_to_trade_strategy());
    // The canonical quote keeps the reservation under the configured cap so admission
    // succeeds; the test then proves the registered submit reaches NT.
    let mut strategy = test_strategy_with_economics_source_decision_evidence_and_submit_admission(
        RecordingEconomicsAdmissionSource::cold(),
        Arc::new(RecordingDecisionEvidenceWriter),
        submit_admission.clone(),
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
    let intent = crate::bolt_v3_decision_evidence::BoltV3OrderIntentEvidence::from_compiled_order(
        strategy.config.strategy_id.clone(),
        crate::bolt_v3_decision_evidence::BoltV3OrderIntentKind::Entry,
        price.to_string(),
        &order,
    );

    strategy
        .submit_order_with_decision_evidence(
            intent,
            order,
            BoltV3SubmitContext::with_client_id(ClientId::from("POLYMARKET")),
            test_gross_expected_value(),
        )
        .expect("provider-limited admission should reach NT submit");
    assert_eq!(submit_admission.admitted_order_count(), 1);
}

#[test]
fn submit_context_routes_non_empty_nt_params_to_submit_order() {
    let submit_admission = submit_admission_with_provider_cap(
        Decimal::new(1, 0),
        Arc::new(RecordingDecisionEvidenceWriter),
    );
    let instrument_id = selected_entry_instrument(&ready_to_trade_strategy());
    // The canonical quote leaves the reservation under the configured cap; this test
    // exercises submit-param routing, not economics aggregation.
    let mut strategy = test_strategy_with_economics_source_decision_evidence_and_submit_admission(
        RecordingEconomicsAdmissionSource::cold(),
        Arc::new(RecordingDecisionEvidenceWriter),
        submit_admission.clone(),
    );
    register_test_strategy_with_instrument(&mut strategy, &instrument_id);
    let (risk_handler, risk_messages) =
        get_typed_into_message_saving_handler::<TradingCommand>(None);
    msgbus::register_trading_command_endpoint(
        MessagingSwitchboard::risk_engine_queue_execute(),
        risk_handler,
    );

    let quantity = Quantity::new(1.0, 2);
    let price = Price::new(0.50, 2);
    let client_order_id = ClientOrderId::from("O-19700101-000000-001-001-1");
    let order_side = strategy
        .configured_entry_order_side()
        .expect("test config should carry an entry order side");
    let order = strategy
        .build_configured_entry_order(instrument_id, order_side, quantity, price, client_order_id)
        .expect("entry order should build through NT OrderFactory");
    let mut params = Params::new();
    params.insert(
        strategy.config.strategy_id.to_string(),
        serde_json::Value::Bool(true),
    );
    let intent = crate::bolt_v3_decision_evidence::BoltV3OrderIntentEvidence::from_compiled_order(
        strategy.config.strategy_id.clone(),
        crate::bolt_v3_decision_evidence::BoltV3OrderIntentKind::Entry,
        price.to_string(),
        &order,
    );

    strategy
        .submit_order_with_decision_evidence(
            intent,
            order,
            BoltV3SubmitContext::from_parts(
                Some(ClientId::from("POLYMARKET")),
                None,
                Some(params.clone()),
            ),
            test_gross_expected_value(),
        )
        .expect("non-empty submit params should reach NT submit");

    let messages = risk_messages.get_messages();
    let Some(TradingCommand::SubmitOrder(command)) = messages.first() else {
        panic!("expected one NT SubmitOrder command, got {messages:#?}");
    };
    assert_eq!(messages.len(), 1);
    assert_eq!(command.params.as_ref(), Some(&params));
    assert_eq!(submit_admission.admitted_order_count(), 1);
}

#[test]
fn submit_admission_uses_compiled_limit_order_notional_not_prebuild_intent() {
    let submit_admission = submit_admission_with_provider_cap(
        Decimal::new(1, 0),
        Arc::new(RecordingDecisionEvidenceWriter),
    );
    let instrument_id = selected_entry_instrument(&ready_to_trade_strategy());
    // Canonical test economics isolates the assertion to "compiled order price drives the
    // notional": the compiled 2.0 notional still exceeds the 1.0 cap, while
    // the understated intent price (0.50) must NOT be what is checked.
    let mut strategy = test_strategy_with_economics_source_decision_evidence_and_submit_admission(
        RecordingEconomicsAdmissionSource::cold(),
        Arc::new(RecordingDecisionEvidenceWriter),
        submit_admission.clone(),
    );
    register_test_strategy_with_instrument(&mut strategy, &instrument_id);
    let quantity = Quantity::new(1.0, 2);
    let price = Price::new(2.0, 2);
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
    let mut understated_intent =
        crate::bolt_v3_decision_evidence::BoltV3OrderIntentEvidence::from_compiled_order(
            strategy.config.strategy_id.clone(),
            crate::bolt_v3_decision_evidence::BoltV3OrderIntentKind::Entry,
            price.to_string(),
            &order,
        );
    understated_intent.price = "0.50".to_string();

    let error = strategy
        .submit_order_with_decision_evidence(
            understated_intent,
            order,
            BoltV3SubmitContext::with_client_id(ClientId::from("POLYMARKET")),
            test_gross_expected_value(),
        )
        .expect_err("compiled order notional above cap must reject before NT submit");

    assert!(
        error.to_string().contains("notional cap is exceeded"),
        "{error:#}"
    );
    assert_eq!(submit_admission.admitted_order_count(), 0);
}

#[test]
fn quote_quantity_submit_admission_matches_nt_effective_notional_for_limit_buy() {
    let mut strategy = ready_to_trade_strategy();
    let cache = register_test_strategy(&mut strategy);
    add_active_instruments_to_cache(&strategy, &cache);
    strategy.config.entry_order.order_type = OrderType::Limit;
    strategy.config.entry_order.is_quote_quantity = true;
    let instrument_id = selected_entry_instrument(&strategy);
    set_active_books_best_prices(&mut strategy, 0.24, 0.25);
    cache
        .borrow_mut()
        .add_quote(quote_tick(&instrument_id.to_string(), 0.24, 0.25, 1_200))
        .expect("test cache should accept quote tick");
    let quantity = Quantity::new(25.0, 2);
    let price = Price::new(0.50, 2);
    let client_order_id = ClientOrderId::from("O-19700101-000000-001-QQQ-1");
    let order = strategy
        .build_configured_entry_order(
            instrument_id,
            OrderSide::Buy,
            quantity,
            price,
            client_order_id,
        )
        .expect("quote-quantity limit order should build through the strategy factory path");
    assert!(order.is_quote_quantity());

    let admission = strategy
        .submit_admission_request_from_order(
            &crate::bolt_v3_decision_evidence::BoltV3OrderIntentEvidence::from_compiled_order(
                strategy.config.strategy_id.clone(),
                crate::bolt_v3_decision_evidence::BoltV3OrderIntentKind::Entry,
                price.to_string(),
                &order,
            ),
            &order,
            test_gross_expected_value(),
        )
        .expect("quote-quantity admission should use NT effective notional");

    assert_eq!(
        admission.economics_admission.reservation_basis().amount(),
        Decimal::from_str("50.00").expect("expected decimal should parse")
    );
}

#[test]
fn quote_quantity_sell_limit_submit_admission_floors_to_quote_quantity() {
    let mut strategy = ready_to_trade_strategy();
    let cache = register_test_strategy(&mut strategy);
    add_active_instruments_to_cache(&strategy, &cache);
    strategy.config.entry_order.order_type = OrderType::Limit;
    strategy.config.entry_order.is_quote_quantity = true;
    let instrument_id = selected_entry_instrument(&strategy);
    set_active_books_best_prices(&mut strategy, 0.75, 0.76);
    cache
        .borrow_mut()
        .add_quote(quote_tick(&instrument_id.to_string(), 0.75, 0.76, 1_200))
        .expect("test cache should accept quote tick");
    let submitted_quote_order_quantity = Quantity::new(25.0, 2);
    let limit_price = Price::new(0.50, 2);
    let client_order_id = ClientOrderId::from("O-19700101-000000-001-QQQ-S1");
    let order = strategy
        .build_configured_entry_order(
            instrument_id,
            OrderSide::Sell,
            submitted_quote_order_quantity,
            limit_price,
            client_order_id,
        )
        .expect("quote-quantity sell limit order should build through the strategy factory path");
    assert!(order.is_quote_quantity());

    let admission = strategy
        .submit_admission_request_from_order(
            &crate::bolt_v3_decision_evidence::BoltV3OrderIntentEvidence::from_compiled_order(
                strategy.config.strategy_id.clone(),
                crate::bolt_v3_decision_evidence::BoltV3OrderIntentKind::Entry,
                limit_price.to_string(),
                &order,
            ),
            &order,
            test_gross_expected_value(),
        )
        .expect("quote-quantity sell limit admission should derive from compiled order context");
    let submitted_quote_quantity = Decimal::from_str(order.quantity().to_string().trim())
        .expect("order quantity should parse as quote quantity");

    assert_eq!(
        admission.economics_admission.reservation_basis().amount(),
        submitted_quote_quantity,
        "SELL Limit base reservation must not understate submitted quote quantity when bid exceeds limit price"
    );
}

#[test]
fn quote_quantity_sell_limit_missing_quote_fails_closed() {
    let mut strategy = ready_to_trade_strategy();
    let cache = register_test_strategy(&mut strategy);
    add_active_instruments_to_cache(&strategy, &cache);
    strategy.config.entry_order.order_type = OrderType::Limit;
    strategy.config.entry_order.is_quote_quantity = true;
    let instrument_id = selected_entry_instrument(&strategy);
    set_active_books_best_prices(&mut strategy, 0.75, 0.76);
    let submitted_quote_order_quantity = Quantity::new(25.0, 2);
    let limit_price = Price::new(0.50, 2);
    let client_order_id = ClientOrderId::from("O-19700101-000000-001-QQQ-S2");
    let order = strategy
        .build_configured_entry_order(
            instrument_id,
            OrderSide::Sell,
            submitted_quote_order_quantity,
            limit_price,
            client_order_id,
        )
        .expect("quote-quantity sell limit order should build through the strategy factory path");
    assert!(order.is_quote_quantity());

    let error = strategy
        .submit_admission_request_from_order(
            &crate::bolt_v3_decision_evidence::BoltV3OrderIntentEvidence::from_compiled_order(
                strategy.config.strategy_id.clone(),
                crate::bolt_v3_decision_evidence::BoltV3OrderIntentKind::Entry,
                limit_price.to_string(),
                &order,
            ),
            &order,
            test_gross_expected_value(),
        )
        .expect_err("missing quote authority must fail closed");

    assert!(
        error
            .to_string()
            .contains("cannot derive authoritative quote-quantity notional"),
        "{error:#}"
    );
}

#[test]
fn quote_quantity_sell_limit_missing_context_fails_closed() {
    let mut builder = ready_to_trade_strategy();
    builder.config.entry_order.order_type = OrderType::Limit;
    builder.config.entry_order.is_quote_quantity = true;
    let instrument_id = selected_entry_instrument(&builder);
    set_active_books_best_prices(&mut builder, 0.75, 0.76);
    let submitted_quote_order_quantity = Quantity::new(25.0, 2);
    let limit_price = Price::new(0.50, 2);
    let client_order_id = ClientOrderId::from("O-19700101-000000-001-QQQ-S3");
    let order = builder
        .build_configured_entry_order(
            instrument_id,
            OrderSide::Sell,
            submitted_quote_order_quantity,
            limit_price,
            client_order_id,
        )
        .expect("quote-quantity sell limit order should build through the strategy factory path");
    assert!(order.is_quote_quantity());

    let mut strategy_without_instrument = ready_to_trade_strategy();
    let cache = register_test_strategy(&mut strategy_without_instrument);
    cache
        .borrow_mut()
        .add_quote(quote_tick(&instrument_id.to_string(), 0.75, 0.76, 1_200))
        .expect("test cache should accept quote tick");

    let error = strategy_without_instrument
        .submit_admission_request_from_order(
            &crate::bolt_v3_decision_evidence::BoltV3OrderIntentEvidence::from_compiled_order(
                strategy_without_instrument.config.strategy_id.clone(),
                crate::bolt_v3_decision_evidence::BoltV3OrderIntentKind::Entry,
                limit_price.to_string(),
                &order,
            ),
            &order,
            test_gross_expected_value(),
        )
        .expect_err("missing instrument context must fail closed");

    assert!(
        error.to_string().contains("instrument context"),
        "{error:#}"
    );
}

#[test]
fn quote_quantity_sell_stop_limit_submit_admission_floors_to_quote_quantity() {
    let mut strategy = ready_to_trade_strategy();
    let cache = register_test_strategy(&mut strategy);
    add_active_instruments_to_cache(&strategy, &cache);
    strategy.config.entry_order.order_type = OrderType::StopLimit;
    strategy.config.entry_order.trigger_price = Some(0.52);
    strategy.config.entry_order.trigger_type = Some(TriggerType::LastPrice);
    strategy.config.entry_order.is_quote_quantity = true;
    let instrument_id = selected_entry_instrument(&strategy);
    set_active_books_best_prices(&mut strategy, 0.75, 0.76);
    cache
        .borrow_mut()
        .add_quote(quote_tick(&instrument_id.to_string(), 0.75, 0.76, 1_200))
        .expect("test cache should accept quote tick");
    let submitted_quote_order_quantity = Quantity::new(25.0, 2);
    let limit_price = Price::new(0.50, 2);
    let client_order_id = ClientOrderId::from("O-19700101-000000-001-QQQ-S4");
    let order = strategy
        .build_configured_entry_order(
            instrument_id,
            OrderSide::Sell,
            submitted_quote_order_quantity,
            limit_price,
            client_order_id,
        )
        .expect(
            "quote-quantity sell stop-limit order should build through the strategy factory path",
        );
    assert!(order.is_quote_quantity());
    assert!(matches!(order, OrderAny::StopLimit(_)));

    let admission = strategy
        .submit_admission_request_from_order(
            &crate::bolt_v3_decision_evidence::BoltV3OrderIntentEvidence::from_compiled_order(
                strategy.config.strategy_id.clone(),
                crate::bolt_v3_decision_evidence::BoltV3OrderIntentKind::Entry,
                limit_price.to_string(),
                &order,
            ),
            &order,
            test_gross_expected_value(),
        )
        .expect(
            "quote-quantity sell stop-limit admission should derive from compiled order context",
        );
    let submitted_quote_quantity = Decimal::from_str(order.quantity().to_string().trim())
        .expect("order quantity should parse as quote quantity");

    assert_eq!(
        admission.economics_admission.reservation_basis().amount(),
        submitted_quote_quantity,
        "SELL StopLimit admission must not understate submitted quote quantity when bid exceeds limit price"
    );
    assert!(
        admission
            .economics_admission
            .full_reservation_liability()
            .amount()
            > submitted_quote_quantity,
        "the fixture must prove the sealed economics debit is added to the base reservation"
    );
}

#[test]
fn quote_quantity_sell_stop_limit_missing_quote_fails_closed() {
    let mut strategy = ready_to_trade_strategy();
    let cache = register_test_strategy(&mut strategy);
    add_active_instruments_to_cache(&strategy, &cache);
    strategy.config.entry_order.order_type = OrderType::StopLimit;
    strategy.config.entry_order.trigger_price = Some(0.52);
    strategy.config.entry_order.trigger_type = Some(TriggerType::LastPrice);
    strategy.config.entry_order.is_quote_quantity = true;
    let instrument_id = selected_entry_instrument(&strategy);
    set_active_books_best_prices(&mut strategy, 0.75, 0.76);
    let submitted_quote_order_quantity = Quantity::new(25.0, 2);
    let limit_price = Price::new(0.50, 2);
    let client_order_id = ClientOrderId::from("O-19700101-000000-001-QQQ-S5");
    let order = strategy
        .build_configured_entry_order(
            instrument_id,
            OrderSide::Sell,
            submitted_quote_order_quantity,
            limit_price,
            client_order_id,
        )
        .expect(
            "quote-quantity sell stop-limit order should build through the strategy factory path",
        );
    assert!(order.is_quote_quantity());
    assert!(matches!(order, OrderAny::StopLimit(_)));

    let error = strategy
        .submit_admission_request_from_order(
            &crate::bolt_v3_decision_evidence::BoltV3OrderIntentEvidence::from_compiled_order(
                strategy.config.strategy_id.clone(),
                crate::bolt_v3_decision_evidence::BoltV3OrderIntentKind::Entry,
                limit_price.to_string(),
                &order,
            ),
            &order,
            test_gross_expected_value(),
        )
        .expect_err("missing quote authority must fail closed");

    assert!(
        error
            .to_string()
            .contains("cannot derive authoritative quote-quantity notional"),
        "{error:#}"
    );
}

#[test]
fn quote_quantity_sell_stop_limit_missing_context_fails_closed() {
    let mut builder = ready_to_trade_strategy();
    builder.config.entry_order.order_type = OrderType::StopLimit;
    builder.config.entry_order.trigger_price = Some(0.52);
    builder.config.entry_order.trigger_type = Some(TriggerType::LastPrice);
    builder.config.entry_order.is_quote_quantity = true;
    let instrument_id = selected_entry_instrument(&builder);
    set_active_books_best_prices(&mut builder, 0.75, 0.76);
    let submitted_quote_order_quantity = Quantity::new(25.0, 2);
    let limit_price = Price::new(0.50, 2);
    let client_order_id = ClientOrderId::from("O-19700101-000000-001-QQQ-S6");
    let order = builder
        .build_configured_entry_order(
            instrument_id,
            OrderSide::Sell,
            submitted_quote_order_quantity,
            limit_price,
            client_order_id,
        )
        .expect(
            "quote-quantity sell stop-limit order should build through the strategy factory path",
        );
    assert!(order.is_quote_quantity());
    assert!(matches!(order, OrderAny::StopLimit(_)));

    let mut strategy_without_instrument = ready_to_trade_strategy();
    let cache = register_test_strategy(&mut strategy_without_instrument);
    cache
        .borrow_mut()
        .add_quote(quote_tick(&instrument_id.to_string(), 0.75, 0.76, 1_200))
        .expect("test cache should accept quote tick");

    let error = strategy_without_instrument
        .submit_admission_request_from_order(
            &crate::bolt_v3_decision_evidence::BoltV3OrderIntentEvidence::from_compiled_order(
                strategy_without_instrument.config.strategy_id.clone(),
                crate::bolt_v3_decision_evidence::BoltV3OrderIntentKind::Entry,
                limit_price.to_string(),
                &order,
            ),
            &order,
            test_gross_expected_value(),
        )
        .expect_err("missing instrument context must fail closed");

    assert!(
        error.to_string().contains("instrument context"),
        "{error:#}"
    );
}

#[test]
fn quote_quantity_limit_missing_nt_cache_quote_fails_closed() {
    let mut strategy = ready_to_trade_strategy();
    let cache = register_test_strategy(&mut strategy);
    add_active_instruments_to_cache(&strategy, &cache);
    strategy.config.entry_order.order_type = OrderType::Limit;
    strategy.config.entry_order.is_quote_quantity = true;
    let instrument_id = selected_entry_instrument(&strategy);
    set_active_books_best_prices(&mut strategy, 0.24, 0.25);
    let quantity = Quantity::new(25.0, 2);
    let price = Price::new(0.50, 2);
    let client_order_id = ClientOrderId::from("O-19700101-000000-001-QQQ-2");
    let order = strategy
        .build_configured_entry_order(
            instrument_id,
            OrderSide::Buy,
            quantity,
            price,
            client_order_id,
        )
        .expect("quote-quantity limit order should build through the strategy factory path");
    assert!(order.is_quote_quantity());

    let error = strategy
        .submit_admission_request_from_order(
            &crate::bolt_v3_decision_evidence::BoltV3OrderIntentEvidence::from_compiled_order(
                strategy.config.strategy_id.clone(),
                crate::bolt_v3_decision_evidence::BoltV3OrderIntentKind::Entry,
                price.to_string(),
                &order,
            ),
            &order,
            test_gross_expected_value(),
        )
        .expect_err("missing quote authority must fail closed");

    assert!(
        error
            .to_string()
            .contains("cannot derive authoritative quote-quantity notional"),
        "{error:#}"
    );
}

#[test]
fn quote_quantity_market_submit_admission_uses_submitted_quote_quantity_with_cached_quote() {
    let mut strategy = ready_to_trade_strategy();
    let cache = register_test_strategy(&mut strategy);
    add_active_instruments_to_cache(&strategy, &cache);
    strategy.config.entry_order.order_type = OrderType::Market;
    strategy.config.entry_order.time_in_force = TimeInForce::Ioc;
    strategy.config.entry_order.is_quote_quantity = true;
    let instrument_id = selected_entry_instrument(&strategy);
    let quote = quote_tick(&instrument_id.to_string(), 0.32, 0.33, 1_200);
    let expected_price = quote.ask_price;
    cache
        .borrow_mut()
        .add_quote(quote)
        .expect("test cache should accept quote tick");
    let quantity = Quantity::new(25.019, 3);
    let client_order_id = ClientOrderId::from("O-19700101-000000-001-QQQ-3");
    let order = strategy
        .build_configured_entry_order(
            instrument_id,
            OrderSide::Buy,
            quantity,
            Price::new(0.99, 2),
            client_order_id,
        )
        .expect("quote-quantity market order should build through the strategy factory path");
    assert!(matches!(order, OrderAny::Market(_)));
    assert!(order.is_quote_quantity());
    let instrument = strategy
        .current_instrument(instrument_id)
        .expect("registered instrument should be available");
    let expected_notional = instrument
        .calculate_notional_value(
            instrument.calculate_base_quantity(order.quantity(), expected_price),
            expected_price,
            Some(true),
        )
        .as_decimal();
    let raw_quote_quantity = Decimal::from_str(order.quantity().to_string().trim())
        .expect("order quantity should parse");
    assert_ne!(expected_notional, raw_quote_quantity);

    let admission = strategy
        .submit_admission_request_from_order(
            &crate::bolt_v3_decision_evidence::BoltV3OrderIntentEvidence::from_compiled_order(
                strategy.config.strategy_id.clone(),
                crate::bolt_v3_decision_evidence::BoltV3OrderIntentKind::Entry,
                "0.99".to_string(),
                &order,
            ),
            &order,
            test_gross_expected_value(),
        )
        .expect("quote-quantity market admission should use submitted quote quantity");

    assert_eq!(
        admission.economics_admission.reservation_basis().amount(),
        raw_quote_quantity,
        "Market BUY commits exactly the submitted quote quantity as its base reservation"
    );
    assert!(
        admission
            .economics_admission
            .full_reservation_liability()
            .amount()
            > raw_quote_quantity,
        "the fixture must prove the sealed economics debit is added to the base reservation"
    );
}

#[test]
fn base_quantity_market_entry_admission_values_at_instrument_price_ceiling() {
    // A base-quantity Market entry has no firm limit price, so the venue fill
    // can land anywhere up to the instrument's structural price ceiling. The
    // admission notional must therefore be valued at that ceiling — the only
    // price the venue cannot exceed — not at the reference price the order
    // happens to be priced at. A firm-limit entry (fill <= limit) needs no
    // such adjustment; this test pins the ceiling valuation for the
    // market-style shape that lacks a firm price.
    let mut strategy = ready_to_trade_strategy();
    let cache = register_test_strategy(&mut strategy);
    add_active_instruments_to_cache(&strategy, &cache);
    strategy.config.entry_order.order_type = OrderType::Market;
    strategy.config.entry_order.time_in_force = TimeInForce::Ioc;
    strategy.config.entry_order.is_quote_quantity = false;
    let instrument_id = selected_entry_instrument(&strategy);
    let quantity = Quantity::new(100.0, 2);
    let price = Price::new(0.33, 2);
    let client_order_id = ClientOrderId::from("O-19700101-000000-001-MKT-1");
    let order = strategy
        .build_configured_entry_order(
            instrument_id,
            OrderSide::Buy,
            quantity,
            price,
            client_order_id,
        )
        .expect("base-quantity market order should build through the strategy factory path");
    assert!(matches!(order, OrderAny::Market(_)));
    assert!(!order.is_quote_quantity());

    let admission = strategy
        .submit_admission_request_from_order(
            &crate::bolt_v3_decision_evidence::BoltV3OrderIntentEvidence::from_compiled_order(
                strategy.config.strategy_id.clone(),
                crate::bolt_v3_decision_evidence::BoltV3OrderIntentKind::Entry,
                price.to_string(),
                &order,
            ),
            &order,
            test_gross_expected_value(),
        )
        .expect("base-quantity market admission should value at the instrument price ceiling");

    // The fixture instrument declares max_price = 0.999 (the production NT
    // Polymarket adapter's ceiling), so the cap is valued at the ceiling
    // (0.999 * 100 = 99.9), NOT at the 0.33 reference price (33.00).
    let reservation_basis = admission.economics_admission.reservation_basis().amount();
    assert_eq!(
        reservation_basis,
        Decimal::from_str("99.9").expect("expected decimal should parse"),
        "a market-style base-quantity entry must be valued at qty * the instrument price ceiling"
    );
    assert!(
        reservation_basis > price.as_decimal() * Decimal::from(100u32),
        "the ceiling valuation must bound strictly above the reference-price estimate it replaces"
    );
}

#[test]
fn quote_quantity_market_submit_admission_uses_submitted_quote_quantity_with_cached_trade() {
    let mut strategy = ready_to_trade_strategy();
    let cache = register_test_strategy(&mut strategy);
    add_active_instruments_to_cache(&strategy, &cache);
    strategy.config.entry_order.order_type = OrderType::Market;
    strategy.config.entry_order.time_in_force = TimeInForce::Ioc;
    strategy.config.entry_order.is_quote_quantity = true;
    let instrument_id = selected_entry_instrument(&strategy);
    let trade = trade_tick(&instrument_id.to_string(), 0.33, 1_200);
    let expected_price = trade.price;
    cache
        .borrow_mut()
        .add_trade(trade)
        .expect("test cache should accept trade tick");
    let quantity = Quantity::new(25.019, 3);
    let client_order_id = ClientOrderId::from("O-19700101-000000-001-QQQ-4");
    let order = strategy
        .build_configured_entry_order(
            instrument_id,
            OrderSide::Buy,
            quantity,
            Price::new(0.99, 2),
            client_order_id,
        )
        .expect("quote-quantity market order should build through the strategy factory path");
    assert!(matches!(order, OrderAny::Market(_)));
    assert!(order.is_quote_quantity());
    let instrument = strategy
        .current_instrument(instrument_id)
        .expect("registered instrument should be available");
    let expected_notional = instrument
        .calculate_notional_value(
            instrument.calculate_base_quantity(order.quantity(), expected_price),
            expected_price,
            Some(true),
        )
        .as_decimal();
    let raw_quote_quantity = Decimal::from_str(order.quantity().to_string().trim())
        .expect("order quantity should parse");
    assert_ne!(expected_notional, raw_quote_quantity);

    let admission = strategy
        .submit_admission_request_from_order(
            &crate::bolt_v3_decision_evidence::BoltV3OrderIntentEvidence::from_compiled_order(
                strategy.config.strategy_id.clone(),
                crate::bolt_v3_decision_evidence::BoltV3OrderIntentKind::Entry,
                "0.99".to_string(),
                &order,
            ),
            &order,
            test_gross_expected_value(),
        )
        .expect("quote-quantity market admission should use submitted quote quantity");

    assert_eq!(
        admission.economics_admission.reservation_basis().amount(),
        raw_quote_quantity,
        "Market BUY commits exactly the submitted quote quantity as its base reservation"
    );
    assert!(
        admission
            .economics_admission
            .full_reservation_liability()
            .amount()
            > raw_quote_quantity,
        "the fixture must prove the sealed economics debit is added to the base reservation"
    );
}

#[test]
fn economics_debit_reservation_over_cap_rejects_before_nt_submit() {
    // The raw order notional is 1.00, while the immutable economics quote
    // contributes a 0.25 core debit reservation. A 1.10 cap therefore admits
    // the raw order but rejects the canonical 1.25 reservation.
    let submit_admission = submit_admission_with_provider_cap(
        Decimal::new(110, 2),
        Arc::new(RecordingDecisionEvidenceWriter),
    );
    let instrument_id = selected_entry_instrument(&ready_to_trade_strategy());
    let mut strategy = test_strategy_with_economics_source_decision_evidence_and_submit_admission(
        RecordingEconomicsAdmissionSource::with_core_effect(Decimal::new(-25, 2)),
        Arc::new(RecordingDecisionEvidenceWriter),
        submit_admission.clone(),
    );
    register_test_strategy_with_instrument(&mut strategy, &instrument_id);
    let quantity = Quantity::new(1.0, 2);
    let price = Price::new(1.0, 2);
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
    let intent = crate::bolt_v3_decision_evidence::BoltV3OrderIntentEvidence::from_compiled_order(
        strategy.config.strategy_id.clone(),
        crate::bolt_v3_decision_evidence::BoltV3OrderIntentKind::Entry,
        price.to_string(),
        &order,
    );

    let error = strategy
        .submit_order_with_decision_evidence(
            intent,
            order,
            BoltV3SubmitContext::with_client_id(ClientId::from("POLYMARKET")),
            test_gross_expected_value(),
        )
        .expect_err("economics-adjusted reservation must reject before NT submit");

    assert!(
        error.to_string().contains("notional cap is exceeded"),
        "{error:#}"
    );
    assert_eq!(submit_admission.admitted_order_count(), 0);
}

#[test]
fn exhausted_count_submit_admission_rejects_before_nt_submit() {
    let submit_admission = submit_admission_with_provider_cap(
        Decimal::new(1, 0),
        Arc::new(RecordingDecisionEvidenceWriter),
    );
    submit_admission
        .admit(
            &crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionRequest {
                economics_admission: crate::bolt_v3_economics_runtime::test_economics_admission(
                    Decimal::new(50, 2),
                ),
                strategy_id: "strategy-a".to_string(),
                execution_client_id: "POLYMARKET".to_string(),
                client_order_id: "client-order-0".to_string(),
                instrument_id: "instrument-0".to_string(),
                order_side: OrderSide::Buy,
                order_quantity: Decimal::new(1, 0),
                intent_kind: crate::bolt_v3_submit_admission::BoltV3SubmitIntentKind::Entry,
                lifecycle_policy: crate::bolt_v3_submit_admission::BoltV3SubmitLifecyclePolicy::new(
                    true,
                ),
                risk_reducing_exit_proof: None,
                kill_switch_forced_reduction: None,
                admission_evidence: None,
            },
        )
        .expect("first admission should consume the only slot")
        .commit_submitted();
    let instrument_id = selected_entry_instrument(&ready_to_trade_strategy());
    // Canonical test economics keeps the reservation under the cap so the rejection is
    // the count-exhausted check, not the notional cap.
    let mut strategy = test_strategy_with_economics_source_decision_evidence_and_submit_admission(
        RecordingEconomicsAdmissionSource::cold(),
        Arc::new(RecordingDecisionEvidenceWriter),
        submit_admission.clone(),
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
    let intent = crate::bolt_v3_decision_evidence::BoltV3OrderIntentEvidence::from_compiled_order(
        strategy.config.strategy_id.clone(),
        crate::bolt_v3_decision_evidence::BoltV3OrderIntentKind::Entry,
        price.to_string(),
        &order,
    );

    let error = strategy
        .submit_order_with_decision_evidence(
            intent,
            order,
            BoltV3SubmitContext::with_client_id(ClientId::from("POLYMARKET")),
            test_gross_expected_value(),
        )
        .expect_err("exhausted count cap must reject before NT submit");

    assert!(
        error.to_string().contains("order count cap is exhausted"),
        "{error:#}"
    );
    assert_eq!(submit_admission.admitted_order_count(), 1);
}

#[test]
fn market_quote_quantity_entry_submission_sizes_from_current_book_notional() {
    let mut strategy = ready_to_trade_strategy();
    let cache = register_test_strategy(&mut strategy);
    add_active_instruments_to_cache(&strategy, &cache);
    set_active_books_best_prices(&mut strategy, 0.40, 0.41);
    strategy.config.entry_order.order_type = OrderType::Market;
    strategy.config.entry_order.time_in_force = TimeInForce::Fok;
    strategy.config.entry_order.is_quote_quantity = true;
    strategy.config.order_notional_target = 25.0;
    strategy.config.maximum_position_notional = 25.0;
    strategy.config.risk_lambda = 0.0001;

    let decision = strategy.entry_submission_decision_at(1_200);

    assert_eq!(decision.blocked_reason, None);
    assert_eq!(decision.price, Some(0.41));
    assert_eq!(decision.quantity_value, Some(25.0));

    let instrument_id = decision
        .instrument_id
        .expect("decision should select an entry instrument");
    let instrument = strategy
        .current_instrument(instrument_id)
        .expect("selected entry instrument should be cached");
    let quantity = instrument
        .try_make_qty(
            decision
                .quantity_value
                .expect("quote quantity notional should be present"),
            Some(true),
        )
        .expect("quote quantity should convert to an NT quantity");
    let order = strategy
        .build_configured_entry_order(
            instrument_id,
            decision.order_side.expect("order side should be selected"),
            quantity,
            Price::new(
                decision.price.expect("fallback price should be selected"),
                instrument.price_precision(),
            ),
            ClientOrderId::from("O-19700101-000000-001-025-1"),
        )
        .expect("market/FOK quote-quantity entry order should build");
    assert!(matches!(order, nautilus_model::orders::OrderAny::Market(_)));
    assert!(order.is_quote_quantity());
}

#[test]
fn market_quote_quantity_entry_submission_blocks_below_venue_minimum() {
    let mut strategy = ready_to_trade_strategy();
    register_test_strategy_with_active_instruments(&mut strategy);
    set_active_books_best_prices(&mut strategy, 0.40, 0.41);
    strategy.config.entry_order.order_type = OrderType::Market;
    strategy.config.entry_order.time_in_force = TimeInForce::Fok;
    strategy.config.entry_order.is_quote_quantity = true;
    let venue_minimum =
        crate::bolt_v3_providers::market_quote_buy_min_notional_for_execution_venue(
            strategy.context.execution_venue(),
        )
        .expect("Polymarket market quote BUY minimum must be modeled");
    let below_minimum = venue_minimum / Decimal::from(2_u32);
    strategy.config.order_notional_target = below_minimum
        .to_f64()
        .expect("fixture below-minimum notional should convert");
    strategy.config.maximum_position_notional = strategy.config.order_notional_target;
    strategy.config.risk_lambda = 0.0001;

    let decision = strategy.entry_submission_decision_at(1_200);

    assert_eq!(
        decision.blocked_reason,
        Some("entry_quote_notional_below_venue_minimum")
    );
    assert_eq!(decision.quantity_value, None);
}

#[test]
fn market_if_touched_order_objects_preserve_nt_trigger_price_and_admission() {
    let mut strategy = ready_to_trade_strategy();
    let cache = register_test_strategy(&mut strategy);
    add_active_instruments_to_cache(&strategy, &cache);
    strategy.config.entry_order.order_type = OrderType::MarketIfTouched;
    strategy.config.entry_order.is_quote_quantity = false;
    strategy.config.entry_order.time_in_force = TimeInForce::Gtc;
    strategy.config.entry_order.trigger_price = Some(0.52);
    strategy.config.entry_order.trigger_type = Some(TriggerType::MarkPrice);
    set_active_books_best_prices(&mut strategy, 0.40, 0.41);
    let instrument_id = selected_entry_instrument(&strategy);

    let quantity = Quantity::new(2.0, 2);
    let fallback_price = Price::new(0.40, 2);
    let order = strategy
        .build_configured_entry_order(
            instrument_id,
            OrderSide::Buy,
            quantity,
            fallback_price,
            ClientOrderId::from("O-19700101-000000-001-006-1"),
        )
        .expect("MarketIfTouched order with explicit trigger price should build");

    let admission = strategy
        .submit_admission_request_from_order(
            &BoltV3OrderIntentEvidence::from_compiled_order(
                strategy.config.strategy_id.clone(),
                BoltV3OrderIntentKind::Entry,
                fallback_price.to_string(),
                &order,
            ),
            &order,
            test_gross_expected_value(),
        )
        .expect("MarketIfTouched admission should derive from the instrument price ceiling");

    let OrderAny::MarketIfTouched(order) = order else {
        panic!("MarketIfTouched config should build an NT market-if-touched order");
    };
    assert_eq!(order.order_type(), OrderType::MarketIfTouched);
    assert_eq!(order.time_in_force(), TimeInForce::Gtc);
    assert_eq!(order.trigger_price(), Some(Price::new(0.52, 2)));
    assert_eq!(order.trigger_type(), Some(TriggerType::MarkPrice));
    assert_eq!(order.price(), None);
    assert!(!order.is_post_only());
    assert!(!order.is_reduce_only());
    assert!(!order.is_quote_quantity());
    assert_eq!(
        admission.economics_admission.reservation_basis().amount(),
        Decimal::from_str("1.998").expect("expected decimal should parse"),
        "a market-style MarketIfTouched entry must be valued at qty * the instrument price ceiling (2 * 0.999)"
    );
}

#[test]
fn submit_admission_uses_configured_execution_client_id() {
    let mut strategy = ready_to_trade_strategy();
    let cache = register_test_strategy(&mut strategy);
    add_active_instruments_to_cache(&strategy, &cache);
    let instrument_id = selected_entry_instrument(&strategy);
    let quantity = Quantity::new(2.0, 2);
    let price = Price::new(0.50, 2);
    let order = strategy
        .build_configured_entry_order(
            instrument_id,
            OrderSide::Buy,
            quantity,
            price,
            ClientOrderId::from("O-19700101-000000-001-HL-1"),
        )
        .expect("configured entry order should build");
    let intent = BoltV3OrderIntentEvidence::from_compiled_order(
        strategy.config.strategy_id.clone(),
        BoltV3OrderIntentKind::Entry,
        price.to_string(),
        &order,
    );

    let admission = strategy
        .submit_admission_request_from_order(&intent, &order, test_gross_expected_value())
        .expect("entry intent should map into submit admission");

    assert_eq!(
        admission.execution_client_id,
        fixture_execution_venue().as_str()
    );
}

#[test]
fn market_if_touched_gtd_order_objects_preserve_nt_expire_time() {
    let mut strategy = ready_to_trade_strategy();
    let expire_time = nautilus_core::UnixNanos::from(4_102_444_800_000_000_000_u64);
    strategy.config.entry_order.order_type = OrderType::MarketIfTouched;
    strategy.config.entry_order.time_in_force = TimeInForce::Gtd;
    strategy.config.entry_order.trigger_price = Some(0.52);
    strategy.config.entry_order.expire_time_unix_nanos = Some(expire_time.as_u64());

    let instrument_id = selected_entry_instrument(&strategy);
    let quantity = Quantity::new(2.0, 2);
    let fallback_price = Price::new(0.40, 2);
    let order = strategy
        .build_configured_entry_order(
            instrument_id,
            OrderSide::Buy,
            quantity,
            fallback_price,
            ClientOrderId::from("O-19700101-000000-001-008-1"),
        )
        .expect("MarketIfTouched GTD order with explicit expiry should build");

    let OrderAny::MarketIfTouched(order) = order else {
        panic!("MarketIfTouched GTD config should build an NT market-if-touched order");
    };
    assert_eq!(order.order_type(), OrderType::MarketIfTouched);
    assert_eq!(order.time_in_force(), TimeInForce::Gtd);
    assert_eq!(order.trigger_price(), Some(Price::new(0.52, 2)));
    assert_eq!(order.expire_time(), Some(expire_time));
}

#[test]
fn post_only_exit_submission_price_uses_passive_book_price() {
    let mut strategy = ready_to_trade_strategy();
    strategy.config.exit_order.order_type = OrderType::Limit;
    strategy.config.exit_order.time_in_force = TimeInForce::Gtc;
    strategy.config.exit_order.is_post_only = true;
    strategy.config.exit_order.is_quote_quantity = false;
    let instrument_id = selected_entry_instrument(&strategy);
    let open_position = materialize_configured_position(
        &mut strategy,
        instrument_id,
        PositionId::from("P-EXIT-001"),
        Quantity::new(10.0, 2),
        0.450,
    );
    let expected_passive_price = open_position.book.best_ask;
    set_managed_position(
        &mut strategy,
        open_position,
        ManagedPositionOrigin::StrategyEntry,
    );

    let order_config = strategy
        .normal_exit_order_execution_config()
        .expect("normal exit config should be valid");
    let (order_side, price) = strategy
        .current_exit_order_for_open_position_with_config(&order_config)
        .expect("post-only exit should resolve from the managed position book");

    assert_eq!(order_side, OrderSide::Sell);
    assert_eq!(Some(price), expected_passive_price);
}

#[test]
fn exit_quote_quantity_config_is_blocked_before_base_position_quantity_is_used() {
    let mut strategy = ready_to_trade_strategy();
    strategy.active.phase = SelectionPhase::Freeze;
    strategy.config.forced_exit_order.is_quote_quantity = true;
    strategy
        .pricing
        .seed_ready_realized_vol(Some("<SOURCE_ID>".to_string()), 2.5, 1_200);
    let instrument_id = selected_entry_instrument(&strategy);
    let open_position = materialize_configured_position(
        &mut strategy,
        instrument_id,
        PositionId::from("P-QUOTE-EXIT-001"),
        Quantity::new(10.0, 2),
        0.450,
    );
    set_managed_position(
        &mut strategy,
        open_position,
        ManagedPositionOrigin::StrategyEntry,
    );

    let decision = strategy.exit_submission_decision_at(1_200);

    assert_eq!(
        decision.blocked_reason,
        Some("exit_quote_quantity_unsupported")
    );
    assert_eq!(decision.quantity, None);
    assert_eq!(decision.is_quote_quantity, None);
}

#[test]
fn exit_quote_quantity_order_build_is_rejected_before_nt_factory() {
    let mut strategy = ready_to_trade_strategy();
    strategy.config.exit_order.is_quote_quantity = true;
    let instrument_id = selected_entry_instrument(&strategy);
    let error = strategy
        .build_configured_exit_order(
            instrument_id,
            OrderSide::Sell,
            Quantity::new(10.0, 2),
            Price::new(0.50, 2),
            ClientOrderId::from("O-19700101-000000-001-QQE-1"),
        )
        .expect_err("exit quote-quantity should fail before NT factory construction");

    assert!(
        error.to_string().contains("exit_is_quote_quantity"),
        "{error:#}"
    );
}

#[test]
fn reduce_only_entry_order_build_is_rejected_before_nt_factory() {
    let mut strategy = ready_to_trade_strategy();
    strategy.config.entry_order.is_reduce_only = true;
    let instrument_id = selected_entry_instrument(&strategy);
    let error = strategy
        .build_configured_entry_order(
            instrument_id,
            OrderSide::Buy,
            Quantity::new(10.0, 2),
            Price::new(0.50, 2),
            ClientOrderId::from("O-19700101-000000-001-ROE-1"),
        )
        .expect_err("reduce-only entry should fail before NT factory construction");

    assert!(
        error.to_string().contains("entry_is_reduce_only"),
        "{error:#}"
    );
}

#[test]
fn forced_flat_exit_uses_forced_exit_order_when_normal_exit_is_post_only() {
    let mut strategy = ready_to_trade_strategy();
    strategy.active.phase = SelectionPhase::Freeze;
    strategy.config.forced_exit_order.order_type = OrderType::Market;
    strategy.config.forced_exit_order.time_in_force = TimeInForce::Ioc;
    strategy.config.forced_exit_order.is_post_only = false;
    strategy.config.forced_exit_order.is_reduce_only = false;
    set_active_books_best_prices(&mut strategy, 0.44, 0.45);
    let instrument_id = selected_entry_instrument(&strategy);
    let open_position = materialize_configured_position(
        &mut strategy,
        instrument_id,
        PositionId::from("P-FORCED-EXIT-001"),
        Quantity::new(10.0, 2),
        0.450,
    );
    set_managed_position(
        &mut strategy,
        open_position,
        ManagedPositionOrigin::StrategyEntry,
    );

    let decision = strategy.exit_submission_decision_at(1_200);

    assert_eq!(decision.forced_flat_reasons, vec![ForcedFlatReason::Freeze]);
    assert_eq!(decision.order_type, Some(OrderType::Market));
    assert_eq!(decision.time_in_force, Some(TimeInForce::Ioc));
    assert_eq!(decision.is_post_only, Some(false));
    assert_eq!(decision.is_reduce_only, Some(false));
    assert_eq!(decision.price, Some(0.44));
}

#[test]
fn forced_flat_exit_order_object_uses_configured_ioc_market_shape() {
    let mut strategy = ready_to_trade_strategy();
    register_test_strategy_with_active_instruments(&mut strategy);
    strategy.active.phase = SelectionPhase::Freeze;
    strategy.config.exit_order.order_type = OrderType::Limit;
    strategy.config.exit_order.time_in_force = TimeInForce::Gtc;
    strategy.config.exit_order.is_post_only = true;
    strategy.config.forced_exit_order = strategy.config.exit_order.clone();
    strategy.config.forced_exit_order.order_type = OrderType::Market;
    strategy.config.forced_exit_order.time_in_force = TimeInForce::Ioc;
    strategy.config.forced_exit_order.is_post_only = false;
    strategy.config.forced_exit_order.is_reduce_only = false;
    let instrument_id = selected_entry_instrument(&strategy);
    let open_position = materialize_configured_position(
        &mut strategy,
        instrument_id,
        PositionId::from("P-FORCED-EXIT-ORDER-001"),
        Quantity::new(10.0, 2),
        0.450,
    );
    set_managed_position(
        &mut strategy,
        open_position,
        ManagedPositionOrigin::StrategyEntry,
    );
    let decision = strategy.exit_submission_decision_at(1_200);
    let price = Price::new(
        decision
            .price
            .expect("forced-flat decision should choose price"),
        strategy
            .current_instrument(instrument_id)
            .expect("test instrument should be registered")
            .price_precision(),
    );

    let order = strategy
        .build_exit_order_with_execution_config(
            decision
                .execution_config()
                .expect("forced-flat decision should carry order config"),
            instrument_id,
            decision.order_side.expect("forced-flat should choose side"),
            decision
                .quantity
                .expect("forced-flat should choose quantity"),
            price,
            ClientOrderId::from("O-19700101-000000-001-005-1"),
        )
        .expect("forced-flat market exit order should build");

    let OrderAny::Market(order) = order else {
        panic!("forced-flat exit should build an NT market order");
    };
    assert_eq!(order.order_type(), OrderType::Market);
    assert_eq!(order.time_in_force(), TimeInForce::Ioc);
    assert_eq!(order.price(), None);
    assert!(!order.is_reduce_only());
    assert!(!order.is_quote_quantity());
}

#[test]
fn forced_flat_exit_order_object_uses_configured_forced_exit_template() {
    let mut strategy = ready_to_trade_strategy();
    let cache = register_test_strategy(&mut strategy);
    add_active_instruments_to_cache(&strategy, &cache);
    strategy.active.phase = SelectionPhase::Freeze;
    strategy.config.exit_order.order_type = OrderType::Market;
    strategy.config.exit_order.time_in_force = TimeInForce::Ioc;
    strategy.config.exit_order.is_post_only = false;
    strategy.config.forced_exit_order = strategy.config.exit_order.clone();
    strategy.config.forced_exit_order.order_type = OrderType::Limit;
    strategy.config.forced_exit_order.time_in_force = TimeInForce::Gtc;
    strategy.config.forced_exit_order.is_post_only = true;
    strategy.config.forced_exit_order.is_reduce_only = true;
    let instrument_id = selected_entry_instrument(&strategy);
    let open_position = materialize_configured_position(
        &mut strategy,
        instrument_id,
        PositionId::from("P-FORCED-EXIT-CONFIGURED-001"),
        Quantity::new(10.0, 2),
        0.450,
    );
    set_managed_position(
        &mut strategy,
        open_position,
        ManagedPositionOrigin::StrategyEntry,
    );
    let decision = strategy.exit_submission_decision_at(1_200);
    let price = Price::new(
        decision
            .price
            .expect("forced-flat decision should choose price"),
        strategy
            .current_instrument(instrument_id)
            .expect("test instrument should be registered")
            .price_precision(),
    );

    let order = strategy
        .build_exit_order_with_execution_config(
            decision
                .execution_config()
                .expect("forced-flat decision should carry order config"),
            instrument_id,
            decision.order_side.expect("forced-flat should choose side"),
            decision
                .quantity
                .expect("forced-flat should choose quantity"),
            price,
            ClientOrderId::from("O-19700101-000000-001-006-1"),
        )
        .expect("configured forced-flat exit order should build");

    let OrderAny::Limit(order) = order else {
        panic!("forced-flat exit should use configured NT order type");
    };
    assert_eq!(order.order_type(), OrderType::Limit);
    assert_eq!(order.time_in_force(), TimeInForce::Gtc);
    assert_eq!(order.price(), Some(price));
    assert!(order.is_post_only());
    assert!(order.is_reduce_only());
    assert!(!order.is_quote_quantity());
}

#[test]
fn post_only_maker_order_objects_preserve_nt_limit_gtc_fields() {
    let mut strategy = ready_to_trade_strategy();
    configure_limit_base_entry_order(&mut strategy);
    let _cache = register_test_strategy(&mut strategy);
    strategy.config.entry_order.time_in_force = TimeInForce::Gtc;
    strategy.config.entry_order.is_post_only = true;
    strategy.config.exit_order.order_type = OrderType::Limit;
    strategy.config.exit_order.time_in_force = TimeInForce::Gtc;
    strategy.config.exit_order.is_post_only = true;

    let instrument_id = selected_entry_instrument(&strategy);
    let quantity = Quantity::new(1.0, 2);
    let entry_price = Price::new(0.40, 2);
    let entry_order = strategy
        .build_configured_entry_order(
            instrument_id,
            OrderSide::Buy,
            quantity,
            entry_price,
            ClientOrderId::from("O-19700101-000000-001-001-1"),
        )
        .expect("maker entry order should build");
    assert_limit_gtc_post_only_order(entry_order, OrderSide::Buy, entry_price);

    let exit_price = Price::new(0.45, 2);
    let exit_order = strategy
        .build_configured_exit_order(
            instrument_id,
            OrderSide::Sell,
            quantity,
            exit_price,
            ClientOrderId::from("O-19700101-000000-001-002-1"),
        )
        .expect("maker exit order should build");
    assert_limit_gtc_post_only_order(exit_order, OrderSide::Sell, exit_price);
}

#[test]
fn gtd_limit_order_objects_preserve_nt_expire_time() {
    let mut strategy = ready_to_trade_strategy();
    configure_limit_base_entry_order(&mut strategy);
    let _cache = register_test_strategy(&mut strategy);
    let expire_time = nautilus_core::UnixNanos::from(4_102_444_800_000_000_000_u64);
    strategy.config.entry_order.time_in_force = TimeInForce::Gtd;
    strategy.config.entry_order.expire_time_unix_nanos = Some(expire_time.as_u64());

    let instrument_id = selected_entry_instrument(&strategy);
    let quantity = Quantity::new(1.0, 2);
    let price = Price::new(0.40, 2);
    let order = strategy
        .build_configured_entry_order(
            instrument_id,
            OrderSide::Buy,
            quantity,
            price,
            ClientOrderId::from("O-19700101-000000-001-003-1"),
        )
        .expect("GTD limit order with explicit expiry should build");

    let OrderAny::Limit(order) = order else {
        panic!("GTD limit config should build an NT limit order");
    };
    assert_eq!(order.time_in_force(), TimeInForce::Gtd);
    assert_eq!(order.expire_time(), Some(expire_time));
}

#[test]
fn non_gtd_limit_order_objects_preserve_nt_expire_time() {
    let mut strategy = ready_to_trade_strategy();
    configure_limit_base_entry_order(&mut strategy);
    let _cache = register_test_strategy(&mut strategy);
    let expire_time = nautilus_core::UnixNanos::from(4_102_444_800_000_000_000_u64);
    strategy.config.entry_order.time_in_force = TimeInForce::Gtc;
    strategy.config.entry_order.expire_time_unix_nanos = Some(expire_time.as_u64());
    let instrument_id = selected_entry_instrument(&strategy);

    let order = strategy
        .build_configured_entry_order(
            instrument_id,
            OrderSide::Buy,
            Quantity::new(1.0, 2),
            Price::new(0.40, 2),
            ClientOrderId::from("O-19700101-000000-001-020-1"),
        )
        .expect("non-GTD limit expiry should pass through to NT");

    let OrderAny::Limit(order) = order else {
        panic!("non-GTD limit config should build an NT limit order");
    };
    assert_eq!(order.time_in_force(), TimeInForce::Gtc);
    assert_eq!(order.expire_time(), Some(expire_time));
}

#[test]
fn stop_market_order_objects_preserve_nt_trigger_price_and_admission() {
    let mut strategy = ready_to_trade_strategy();
    let cache = register_test_strategy(&mut strategy);
    add_active_instruments_to_cache(&strategy, &cache);
    strategy.config.entry_order.order_type = OrderType::StopMarket;
    strategy.config.entry_order.is_quote_quantity = false;
    strategy.config.entry_order.time_in_force = TimeInForce::Gtc;
    strategy.config.entry_order.trigger_price = Some(0.52);
    strategy.config.entry_order.trigger_type = Some(TriggerType::LastPrice);

    let instrument_id = selected_entry_instrument(&strategy);
    let quantity = Quantity::new(2.0, 2);
    let admission_price = Price::new(0.40, 2);
    let order = strategy
        .build_configured_entry_order(
            instrument_id,
            OrderSide::Buy,
            quantity,
            admission_price,
            ClientOrderId::from("O-19700101-000000-001-005-1"),
        )
        .expect("StopMarket order with explicit trigger price should build");

    let admission = strategy
        .submit_admission_request_from_order(
            &BoltV3OrderIntentEvidence::from_compiled_order(
                strategy.config.strategy_id.clone(),
                BoltV3OrderIntentKind::Entry,
                admission_price.to_string(),
                &order,
            ),
            &order,
            test_gross_expected_value(),
        )
        .expect("StopMarket admission should derive from the instrument price ceiling");

    let OrderAny::StopMarket(order) = order else {
        panic!("StopMarket config should build an NT stop-market order");
    };
    assert_eq!(order.order_type(), OrderType::StopMarket);
    assert_eq!(order.time_in_force(), TimeInForce::Gtc);
    assert_eq!(order.trigger_price(), Some(Price::new(0.52, 2)));
    assert_eq!(order.trigger_type(), Some(TriggerType::LastPrice));
    assert_eq!(order.price(), None);
    assert!(!order.is_reduce_only());
    assert!(!order.is_quote_quantity());
    assert_eq!(
        admission.economics_admission.reservation_basis().amount(),
        Decimal::from_str("1.998").expect("expected decimal should parse"),
        "a market-style StopMarket entry must be valued at qty * the instrument price ceiling (2 * 0.999)"
    );
}

#[test]
fn triggered_order_objects_preserve_nt_trigger_instrument_id() {
    let mut raw = valid_raw_config();
    let exit_order = raw
        .as_table_mut()
        .expect("valid config must be a table")
        .get_mut("exit_order")
        .expect("valid config should include exit_order")
        .as_table_mut()
        .expect("exit_order should be a table");
    exit_order.insert(
        "order_type".to_string(),
        Value::String("stop_market".to_string()),
    );
    exit_order.insert(
        "time_in_force".to_string(),
        Value::String("gtc".to_string()),
    );
    exit_order.insert("trigger_price".to_string(), Value::Float(0.52));
    exit_order.insert(
        "trigger_instrument_id".to_string(),
        Value::String("TRIGGER.SOURCE".to_string()),
    );
    let config = BinaryOracleEdgeTakerBuilder::parse_config(&raw)
        .expect("trigger_instrument_id should parse through runtime config");
    let context =
        test_build_context_with_economics_source(RecordingEconomicsAdmissionSource::cold());
    let mut strategy = BinaryOracleEdgeTaker::new(config, context);
    let _cache = register_test_strategy(&mut strategy);
    let trigger_instrument_id = InstrumentId::from("TRIGGER.SOURCE");
    let instrument_id = selected_entry_instrument(&ready_to_trade_strategy());

    let order = strategy
        .build_configured_exit_order(
            instrument_id,
            OrderSide::Sell,
            Quantity::new(2.0, 2),
            Price::new(0.40, 2),
            ClientOrderId::from("O-19700101-000000-001-021-1"),
        )
        .expect("triggered order should build with NT trigger_instrument_id");

    let OrderAny::StopMarket(order) = order else {
        panic!("StopMarket config should build an NT stop-market order");
    };
    assert_eq!(order.trigger_instrument_id(), Some(trigger_instrument_id));
}

#[test]
fn non_triggered_order_rejects_trigger_instrument_id_before_factory() {
    let mut raw = valid_raw_config();
    let exit_order = raw
        .as_table_mut()
        .expect("valid config must be a table")
        .get_mut("exit_order")
        .expect("valid config should include exit_order")
        .as_table_mut()
        .expect("exit_order should be a table");
    exit_order.insert(
        "trigger_instrument_id".to_string(),
        Value::String("TRIGGER.SOURCE".to_string()),
    );
    let config = BinaryOracleEdgeTakerBuilder::parse_config(&raw)
        .expect("trigger_instrument_id should parse through runtime config");
    let context =
        test_build_context_with_economics_source(RecordingEconomicsAdmissionSource::cold());
    let mut strategy = BinaryOracleEdgeTaker::new(config, context);
    let _cache = register_test_strategy(&mut strategy);
    let instrument_id = selected_entry_instrument(&ready_to_trade_strategy());

    let error = strategy
        .build_configured_exit_order(
            instrument_id,
            OrderSide::Sell,
            Quantity::new(2.0, 2),
            Price::new(0.40, 2),
            ClientOrderId::from("O-19700101-000000-001-022-1"),
        )
        .expect_err("non-triggered order must not silently carry trigger_instrument_id");

    assert!(
        error.to_string().contains("trigger_instrument_id"),
        "{error}"
    );
}

#[test]
fn stop_limit_order_objects_preserve_nt_price_trigger_and_admission() {
    let mut strategy = ready_to_trade_strategy();
    let cache = register_test_strategy(&mut strategy);
    add_active_instruments_to_cache(&strategy, &cache);
    let expire_time = nautilus_core::UnixNanos::from(4_102_444_800_000_000_000_u64);
    strategy.config.entry_order.order_type = OrderType::StopLimit;
    strategy.config.entry_order.time_in_force = TimeInForce::Gtd;
    strategy.config.entry_order.expire_time_unix_nanos = Some(expire_time.as_u64());
    strategy.config.entry_order.trigger_price = Some(0.52);
    strategy.config.entry_order.trigger_type = Some(TriggerType::LastPrice);
    strategy.config.entry_order.is_post_only = true;
    strategy.config.entry_order.is_quote_quantity = false;
    strategy.config.exit_order.order_type = OrderType::StopLimit;
    strategy.config.exit_order.time_in_force = TimeInForce::Gtd;
    strategy.config.exit_order.expire_time_unix_nanos = Some(expire_time.as_u64());
    strategy.config.exit_order.trigger_price = Some(0.48);
    strategy.config.exit_order.trigger_type = Some(TriggerType::MarkPrice);
    strategy.config.exit_order.is_post_only = true;
    strategy.config.exit_order.is_quote_quantity = false;

    let instrument_id = selected_entry_instrument(&strategy);
    let quantity = Quantity::new(2.0, 2);
    let price = Price::new(0.40, 2);
    let order = strategy
        .build_configured_entry_order(
            instrument_id,
            OrderSide::Buy,
            quantity,
            price,
            ClientOrderId::from("O-19700101-000000-001-006-1"),
        )
        .expect("StopLimit order with explicit trigger price should build");

    let intent = BoltV3OrderIntentEvidence::from_compiled_order(
        strategy.config.strategy_id.clone(),
        BoltV3OrderIntentKind::Entry,
        price.to_string(),
        &order,
    );
    let admission = strategy
        .submit_admission_request_from_order(&intent, &order, test_gross_expected_value())
        .expect("StopLimit admission should derive from the compiled NT order");

    let OrderAny::StopLimit(order) = order else {
        panic!("StopLimit config should build an NT stop-limit order");
    };
    assert_eq!(order.order_type(), OrderType::StopLimit);
    assert_eq!(order.time_in_force(), TimeInForce::Gtd);
    assert_eq!(order.price(), Some(price));
    assert_eq!(order.trigger_price(), Some(Price::new(0.52, 2)));
    assert_eq!(order.trigger_type(), Some(TriggerType::LastPrice));
    assert_eq!(order.expire_time(), Some(expire_time));
    assert!(order.is_post_only());
    assert!(!order.is_reduce_only());
    assert!(!order.is_quote_quantity());
    assert_eq!(
        admission.economics_admission.reservation_basis().amount(),
        Decimal::from_str("0.800").expect("expected decimal should parse")
    );

    let exit_price = Price::new(0.45, 2);
    let exit_order = strategy
        .build_configured_exit_order(
            instrument_id,
            OrderSide::Sell,
            quantity,
            exit_price,
            ClientOrderId::from("O-19700101-000000-001-007-1"),
        )
        .expect("StopLimit exit order with explicit trigger price should build");

    let OrderAny::StopLimit(exit_order) = exit_order else {
        panic!("StopLimit exit config should build an NT stop-limit order");
    };
    assert_eq!(exit_order.order_side(), OrderSide::Sell);
    assert_eq!(exit_order.order_type(), OrderType::StopLimit);
    assert_eq!(exit_order.time_in_force(), TimeInForce::Gtd);
    assert_eq!(exit_order.price(), Some(exit_price));
    assert_eq!(exit_order.trigger_price(), Some(Price::new(0.48, 2)));
    assert_eq!(exit_order.trigger_type(), Some(TriggerType::MarkPrice));
    assert_eq!(exit_order.expire_time(), Some(expire_time));
    assert!(exit_order.is_post_only());
    assert!(!exit_order.is_reduce_only());
    assert!(!exit_order.is_quote_quantity());
}

#[test]
fn limit_if_touched_order_objects_preserve_nt_price_trigger_and_admission() {
    let mut strategy = ready_to_trade_strategy();
    let cache = register_test_strategy(&mut strategy);
    add_active_instruments_to_cache(&strategy, &cache);
    let expire_time = nautilus_core::UnixNanos::from(4_102_444_800_000_000_000_u64);
    strategy.config.entry_order.order_type = OrderType::LimitIfTouched;
    strategy.config.entry_order.time_in_force = TimeInForce::Gtd;
    strategy.config.entry_order.expire_time_unix_nanos = Some(expire_time.as_u64());
    strategy.config.entry_order.trigger_price = Some(0.39);
    strategy.config.entry_order.trigger_type = Some(TriggerType::LastPrice);
    strategy.config.entry_order.is_post_only = true;
    strategy.config.exit_order.order_type = OrderType::LimitIfTouched;
    strategy.config.exit_order.time_in_force = TimeInForce::Gtd;
    strategy.config.exit_order.expire_time_unix_nanos = Some(expire_time.as_u64());
    strategy.config.exit_order.trigger_price = Some(0.46);
    strategy.config.exit_order.trigger_type = Some(TriggerType::MarkPrice);
    strategy.config.exit_order.is_post_only = true;

    let instrument_id = selected_entry_instrument(&strategy);
    let quantity = Quantity::new(2.0, 2);
    let price = Price::new(0.40, 2);
    let order = strategy
        .build_configured_entry_order(
            instrument_id,
            OrderSide::Buy,
            quantity,
            price,
            ClientOrderId::from("O-19700101-000000-001-009-1"),
        )
        .expect("LimitIfTouched entry order with explicit trigger price should build");

    let intent = BoltV3OrderIntentEvidence::from_compiled_order(
        strategy.config.strategy_id.clone(),
        BoltV3OrderIntentKind::Entry,
        price.to_string(),
        &order,
    );
    let admission = strategy
        .submit_admission_request_from_order(&intent, &order, test_gross_expected_value())
        .expect("LimitIfTouched admission should derive from the compiled NT order");

    let OrderAny::LimitIfTouched(order) = order else {
        panic!("LimitIfTouched config should build an NT limit-if-touched order");
    };
    assert_eq!(order.order_side(), OrderSide::Buy);
    assert_eq!(order.order_type(), OrderType::LimitIfTouched);
    assert_eq!(order.time_in_force(), TimeInForce::Gtd);
    assert_eq!(order.price(), Some(price));
    assert_eq!(order.trigger_price(), Some(Price::new(0.39, 2)));
    assert_eq!(order.trigger_type(), Some(TriggerType::LastPrice));
    assert_eq!(order.expire_time(), Some(expire_time));
    assert!(order.is_post_only());
    assert!(!order.is_reduce_only());
    assert!(order.is_quote_quantity());
    assert_eq!(
        admission.economics_admission.reservation_basis().amount(),
        Decimal::from_str("2.00").expect("expected decimal should parse")
    );

    let exit_price = Price::new(0.45, 2);
    let exit_order = strategy
        .build_configured_exit_order(
            instrument_id,
            OrderSide::Sell,
            quantity,
            exit_price,
            ClientOrderId::from("O-19700101-000000-001-010-1"),
        )
        .expect("LimitIfTouched exit order with explicit trigger price should build");

    let OrderAny::LimitIfTouched(exit_order) = exit_order else {
        panic!("LimitIfTouched exit config should build an NT limit-if-touched order");
    };
    assert_eq!(exit_order.order_side(), OrderSide::Sell);
    assert_eq!(exit_order.order_type(), OrderType::LimitIfTouched);
    assert_eq!(exit_order.time_in_force(), TimeInForce::Gtd);
    assert_eq!(exit_order.price(), Some(exit_price));
    assert_eq!(exit_order.trigger_price(), Some(Price::new(0.46, 2)));
    assert_eq!(exit_order.trigger_type(), Some(TriggerType::MarkPrice));
    assert_eq!(exit_order.expire_time(), Some(expire_time));
    assert!(exit_order.is_post_only());
    assert!(!exit_order.is_reduce_only());
    assert!(!exit_order.is_quote_quantity());
}

#[test]
fn trailing_stop_market_order_objects_preserve_nt_trailing_fields_and_admission() {
    let mut strategy = ready_to_trade_strategy();
    let cache = register_test_strategy(&mut strategy);
    add_active_instruments_to_cache(&strategy, &cache);
    let expire_time = nautilus_core::UnixNanos::from(4_102_444_800_000_000_000_u64);
    strategy.config.entry_order.order_type = OrderType::TrailingStopMarket;
    strategy.config.entry_order.time_in_force = TimeInForce::Gtd;
    strategy.config.entry_order.expire_time_unix_nanos = Some(expire_time.as_u64());
    strategy.config.entry_order.trigger_price = Some(0.52);
    strategy.config.entry_order.activation_price = Some(0.47);
    strategy.config.entry_order.trigger_type = Some(TriggerType::LastPrice);
    strategy.config.entry_order.trailing_offset = Some(2.5);
    strategy.config.entry_order.trailing_offset_type = Some(TrailingOffsetType::BasisPoints);
    strategy.config.entry_order.is_post_only = false;
    strategy.config.entry_order.is_quote_quantity = false;
    strategy.config.exit_order.order_type = OrderType::TrailingStopMarket;
    strategy.config.exit_order.time_in_force = TimeInForce::Gtd;
    strategy.config.exit_order.expire_time_unix_nanos = Some(expire_time.as_u64());
    strategy.config.exit_order.activation_price = Some(0.48);
    strategy.config.exit_order.trigger_type = Some(TriggerType::MarkPrice);
    strategy.config.exit_order.trailing_offset = Some(3.0);
    strategy.config.exit_order.trailing_offset_type = Some(TrailingOffsetType::Ticks);
    strategy.config.exit_order.is_post_only = false;
    strategy.config.exit_order.is_quote_quantity = false;

    let instrument_id = selected_entry_instrument(&strategy);
    let quantity = Quantity::new(2.0, 2);
    let fallback_price = Price::new(0.40, 2);
    let order = strategy
        .build_configured_entry_order(
            instrument_id,
            OrderSide::Buy,
            quantity,
            fallback_price,
            ClientOrderId::from("O-19700101-000000-001-013-1"),
        )
        .expect("TrailingStopMarket entry order with explicit trailing fields should build");

    let ceiling_price = Decimal::from_str("0.999").expect("fixture ceiling should parse");
    let admission = strategy
        .submit_admission_request_from_order_inner(
            &BoltV3OrderIntentEvidence::from_compiled_order(
                strategy.config.strategy_id.clone(),
                BoltV3OrderIntentKind::Entry,
                fallback_price.to_string(),
                &order,
            ),
            &order,
            test_gross_expected_value(),
            StrategyPlannedFillInput::Exact(vec![BoltV3PlannedFillLeg {
                price: ceiling_price,
                quantity: Decimal::from(2_u32),
            }]),
        )
        .expect("TrailingStopMarket admission should derive from the instrument price ceiling");

    let OrderAny::TrailingStopMarket(order) = order else {
        panic!("TrailingStopMarket config should build an NT trailing-stop-market order");
    };
    assert_eq!(order.order_side(), OrderSide::Buy);
    assert_eq!(order.order_type(), OrderType::TrailingStopMarket);
    assert_eq!(order.time_in_force(), TimeInForce::Gtd);
    assert_eq!(order.price(), None);
    assert_eq!(order.trigger_price(), Some(Price::new(0.52, 2)));
    assert_eq!(order.activation_price(), Some(Price::new(0.47, 2)));
    assert_eq!(order.trigger_type(), Some(TriggerType::LastPrice));
    assert_eq!(order.trailing_offset(), Some(Decimal::new(25, 1)));
    assert_eq!(
        order.trailing_offset_type(),
        Some(TrailingOffsetType::BasisPoints)
    );
    assert_eq!(order.expire_time(), Some(expire_time));
    assert!(!order.is_post_only());
    assert!(!order.is_reduce_only());
    assert!(!order.is_quote_quantity());
    assert_eq!(
        admission.economics_admission.reservation_basis().amount(),
        Decimal::from_str("1.998").expect("expected decimal should parse"),
        "a market-style TrailingStopMarket entry must be valued at qty * the instrument price ceiling (2 * 0.999)"
    );

    let managed_position = materialize_configured_position(
        &mut strategy,
        instrument_id,
        PositionId::from("P-STOP-EXIT"),
        quantity,
        0.450,
    );
    set_managed_position(
        &mut strategy,
        managed_position,
        ManagedPositionOrigin::StrategyEntry,
    );

    let exit_fallback_price = Price::new(0.45, 2);
    let exit_order = strategy
        .build_configured_exit_order(
            instrument_id,
            OrderSide::Sell,
            quantity,
            exit_fallback_price,
            ClientOrderId::from("O-19700101-000000-001-014-1"),
        )
        .expect("TrailingStopMarket exit order with explicit activation price should build");

    // A market-style (price-less) EXIT is valued at the instrument's
    // structural price CEILING (`max_price`) — the universally fail-closed
    // worst case for any side/intent — NOT at its reference/activation price.
    // Valuing it at the activation price (as the pre-A4 code did) was a
    // reference-price estimate, not a structural bound. This is the exit
    // counterpart of the entry ceiling valuation above and must run through
    // the strategy method that carries instrument context.
    let exit_admission = strategy
        .submit_admission_request_from_order(
            &BoltV3OrderIntentEvidence::from_compiled_order(
                strategy.config.strategy_id.clone(),
                BoltV3OrderIntentKind::Exit,
                exit_fallback_price.to_string(),
                &exit_order,
            ),
            &exit_order,
            test_gross_expected_value(),
        )
        .expect("market-style exit admission should derive from the instrument price ceiling");

    let OrderAny::TrailingStopMarket(exit_order) = exit_order else {
        panic!("TrailingStopMarket exit config should build an NT trailing-stop-market order");
    };
    assert_eq!(exit_order.order_side(), OrderSide::Sell);
    assert_eq!(exit_order.order_type(), OrderType::TrailingStopMarket);
    assert_eq!(exit_order.time_in_force(), TimeInForce::Gtd);
    assert_eq!(exit_order.price(), None);
    assert_eq!(exit_order.trigger_price(), None);
    assert_eq!(exit_order.activation_price(), Some(Price::new(0.48, 2)));
    // The fixture instrument declares max_price = 0.999 (the production NT
    // Polymarket adapter's ceiling), so the market-style exit cap is valued
    // at the ceiling (0.999 * 2 = 1.998), strictly ABOVE the 0.48
    // activation-price estimate it replaces (0.96).
    assert_eq!(
        exit_admission
            .economics_admission
            .reservation_basis()
            .amount(),
        Decimal::from_str("1.998").expect("expected decimal should parse"),
        "a market-style exit must be valued at qty * the instrument price ceiling (2 * 0.999)"
    );
    assert!(
        exit_admission
            .economics_admission
            .reservation_basis()
            .amount()
            > Decimal::from_str("0.48").expect("expected decimal should parse")
                * Decimal::from_str(quantity.to_string().trim()).expect("quantity should parse"),
        "the ceiling valuation must bound strictly above the activation-price estimate it replaces"
    );
    assert_eq!(exit_order.trigger_type(), Some(TriggerType::MarkPrice));
    assert_eq!(exit_order.trailing_offset(), Some(Decimal::new(3, 0)));
    assert_eq!(
        exit_order.trailing_offset_type(),
        Some(TrailingOffsetType::Ticks)
    );
    assert_eq!(exit_order.expire_time(), Some(expire_time));
    assert!(!exit_order.is_post_only());
    assert!(!exit_order.is_reduce_only());
    assert!(!exit_order.is_quote_quantity());
}

#[test]
fn trailing_stop_market_order_objects_use_nt_default_types_when_omitted() {
    let mut strategy = ready_to_trade_strategy();
    strategy.config.entry_order.order_type = OrderType::TrailingStopMarket;
    strategy.config.entry_order.time_in_force = TimeInForce::Gtc;
    strategy.config.entry_order.trigger_price = Some(0.52);
    strategy.config.entry_order.trailing_offset = Some(2.5);
    strategy.config.entry_order.is_post_only = false;
    let instrument_id = selected_entry_instrument(&strategy);

    let order = strategy
        .build_configured_entry_order(
            instrument_id,
            OrderSide::Buy,
            Quantity::new(2.0, 2),
            Price::new(0.40, 2),
            ClientOrderId::from("O-19700101-000000-001-019-1"),
        )
        .expect("TrailingStopMarket should use NT defaults for omitted type fields");

    let OrderAny::TrailingStopMarket(order) = order else {
        panic!("TrailingStopMarket config should build an NT trailing-stop-market order");
    };
    assert_eq!(order.trigger_type(), Some(TriggerType::Default));
    assert_eq!(
        order.trailing_offset_type(),
        Some(TrailingOffsetType::Price)
    );
}

#[test]
fn trailing_stop_market_rejects_required_nt_fields_before_factory() {
    let instrument_id = selected_entry_instrument(&ready_to_trade_strategy());
    let quantity = Quantity::new(1.0, 2);
    let price = Price::new(0.40, 2);

    let mut missing_offset = ready_to_trade_strategy();
    missing_offset.config.entry_order.order_type = OrderType::TrailingStopMarket;
    missing_offset.config.entry_order.time_in_force = TimeInForce::Gtc;
    missing_offset.config.entry_order.trigger_price = Some(0.52);
    missing_offset.config.entry_order.trigger_type = Some(TriggerType::LastPrice);
    missing_offset.config.entry_order.trailing_offset_type = Some(TrailingOffsetType::Price);
    let missing_offset_error = missing_offset
        .build_configured_entry_order(
            instrument_id,
            OrderSide::Buy,
            quantity,
            price,
            ClientOrderId::from("O-19700101-000000-001-015-1"),
        )
        .expect_err("TrailingStopMarket without trailing_offset should fail before factory");
    assert!(
        missing_offset_error.to_string().contains("trailing_offset"),
        "{missing_offset_error}"
    );

    for trailing_offset in [0.0, -0.01] {
        let mut invalid_offset = ready_to_trade_strategy();
        invalid_offset.config.entry_order.order_type = OrderType::TrailingStopMarket;
        invalid_offset.config.entry_order.time_in_force = TimeInForce::Gtc;
        invalid_offset.config.entry_order.trigger_price = Some(0.52);
        invalid_offset.config.entry_order.trigger_type = Some(TriggerType::LastPrice);
        invalid_offset.config.entry_order.trailing_offset = Some(trailing_offset);
        invalid_offset.config.entry_order.trailing_offset_type = Some(TrailingOffsetType::Price);
        let invalid_offset_error = invalid_offset
            .build_configured_entry_order(
                instrument_id,
                OrderSide::Buy,
                quantity,
                price,
                ClientOrderId::from("O-19700101-000000-001-016-1"),
            )
            .expect_err(
                "TrailingStopMarket with non-positive trailing_offset should fail before factory",
            );
        assert!(
            invalid_offset_error.to_string().contains("trailing_offset"),
            "{invalid_offset_error}"
        );
    }

    let mut missing_trigger = ready_to_trade_strategy();
    missing_trigger.config.entry_order.order_type = OrderType::TrailingStopMarket;
    missing_trigger.config.entry_order.time_in_force = TimeInForce::Gtc;
    missing_trigger.config.entry_order.trigger_type = Some(TriggerType::LastPrice);
    missing_trigger.config.entry_order.trailing_offset = Some(1.0);
    missing_trigger.config.entry_order.trailing_offset_type = Some(TrailingOffsetType::Price);
    let missing_trigger_error = missing_trigger
        .build_configured_entry_order(
            instrument_id,
            OrderSide::Buy,
            quantity,
            price,
            ClientOrderId::from("O-19700101-000000-001-017-1"),
        )
        .expect_err(
            "TrailingStopMarket without trigger_price or activation_price should fail before factory",
        );
    assert!(
        missing_trigger_error.to_string().contains("trigger_price")
            && missing_trigger_error
                .to_string()
                .contains("activation_price"),
        "{missing_trigger_error}"
    );

    let mut is_post_only = ready_to_trade_strategy();
    is_post_only.config.entry_order.order_type = OrderType::TrailingStopMarket;
    is_post_only.config.entry_order.time_in_force = TimeInForce::Gtc;
    is_post_only.config.entry_order.trigger_price = Some(0.52);
    is_post_only.config.entry_order.trigger_type = Some(TriggerType::LastPrice);
    is_post_only.config.entry_order.trailing_offset = Some(1.0);
    is_post_only.config.entry_order.trailing_offset_type = Some(TrailingOffsetType::Price);
    is_post_only.config.entry_order.is_post_only = true;
    let is_post_only_error = is_post_only
        .build_configured_entry_order(
            instrument_id,
            OrderSide::Buy,
            quantity,
            price,
            ClientOrderId::from("O-19700101-000000-001-018-1"),
        )
        .expect_err("TrailingStopMarket post-only should fail before factory");
    assert!(
        is_post_only_error.to_string().contains("is_post_only"),
        "{is_post_only_error}"
    );
}

#[test]
fn configured_order_build_rejects_nt_model_invalid_tif_before_factory() {
    let mut strategy = ready_to_trade_strategy();
    let _cache = register_test_strategy(&mut strategy);
    let instrument_id = selected_entry_instrument(&strategy);
    let quantity = Quantity::new(1.0, 2);
    let price = Price::new(0.40, 2);

    strategy.config.entry_order.order_type = OrderType::Limit;
    strategy.config.entry_order.time_in_force = TimeInForce::Gtd;
    let limit_error = strategy
        .build_configured_entry_order(
            instrument_id,
            OrderSide::Buy,
            quantity,
            price,
            ClientOrderId::from("O-19700101-000000-001-003-1"),
        )
        .expect_err("limit GTD without expire_time should fail before NT factory");
    assert!(
        limit_error.to_string().contains("expire_time"),
        "{limit_error}"
    );

    strategy.config.entry_order.order_type = OrderType::Market;
    strategy.config.entry_order.time_in_force = TimeInForce::Gtd;
    let market_error = strategy
        .build_configured_entry_order(
            instrument_id,
            OrderSide::Buy,
            quantity,
            price,
            ClientOrderId::from("O-19700101-000000-001-004-1"),
        )
        .expect_err("market GTD should fail before NT factory");
    assert!(
        market_error
            .to_string()
            .contains("GTD not supported for Market orders"),
        "{market_error}"
    );

    strategy.config.entry_order.time_in_force = TimeInForce::Fok;
    strategy.config.entry_order.expire_time_unix_nanos = Some(4_102_444_800_000_000_000_u64);
    let market_expiry_error = strategy
        .build_configured_entry_order(
            instrument_id,
            OrderSide::Buy,
            quantity,
            price,
            ClientOrderId::from("O-19700101-000000-001-005-1"),
        )
        .expect_err("market expire_time should fail before NT factory");
    assert!(
        market_expiry_error
            .to_string()
            .contains("expire_time is not supported for Market orders"),
        "{market_expiry_error}"
    );

    strategy.config.entry_order.expire_time_unix_nanos = None;
    strategy.config.entry_order.order_type = OrderType::StopLimit;
    strategy.config.entry_order.time_in_force = TimeInForce::Gtd;
    strategy.config.entry_order.trigger_price = Some(0.52);
    let stop_limit_error = strategy
        .build_configured_entry_order(
            instrument_id,
            OrderSide::Buy,
            quantity,
            price,
            ClientOrderId::from("O-19700101-000000-001-005-1"),
        )
        .expect_err("StopLimit GTD without expire_time should fail before NT factory");
    assert!(
        stop_limit_error.to_string().contains("expire_time"),
        "{stop_limit_error}"
    );

    strategy.config.entry_order.order_type = OrderType::MarketIfTouched;
    strategy.config.entry_order.time_in_force = TimeInForce::Gtd;
    strategy.config.entry_order.trigger_price = Some(0.52);
    let market_if_touched_error = strategy
        .build_configured_entry_order(
            instrument_id,
            OrderSide::Buy,
            quantity,
            price,
            ClientOrderId::from("O-19700101-000000-001-007-1"),
        )
        .expect_err("MarketIfTouched GTD without expire_time should fail before NT factory");
    assert!(
        market_if_touched_error.to_string().contains("expire_time"),
        "{market_if_touched_error}"
    );

    strategy.config.entry_order.order_type = OrderType::LimitIfTouched;
    strategy.config.entry_order.time_in_force = TimeInForce::Gtd;
    strategy.config.entry_order.trigger_price = Some(0.39);
    let limit_if_touched_error = strategy
        .build_configured_entry_order(
            instrument_id,
            OrderSide::Buy,
            quantity,
            price,
            ClientOrderId::from("O-19700101-000000-001-008-1"),
        )
        .expect_err("LimitIfTouched GTD without expire_time should fail before NT factory");
    assert!(
        limit_if_touched_error.to_string().contains("expire_time"),
        "{limit_if_touched_error}"
    );

    strategy.config.entry_order.order_type = OrderType::TrailingStopMarket;
    strategy.config.entry_order.time_in_force = TimeInForce::Gtd;
    strategy.config.entry_order.trigger_price = Some(0.52);
    strategy.config.entry_order.trigger_type = Some(TriggerType::LastPrice);
    strategy.config.entry_order.trailing_offset = Some(1.0);
    strategy.config.entry_order.trailing_offset_type = Some(TrailingOffsetType::Price);
    let trailing_stop_market_error = strategy
        .build_configured_entry_order(
            instrument_id,
            OrderSide::Buy,
            quantity,
            price,
            ClientOrderId::from("O-19700101-000000-001-009-1"),
        )
        .expect_err("TrailingStopMarket GTD without expire_time should fail before NT factory");
    assert!(
        trailing_stop_market_error
            .to_string()
            .contains("expire_time"),
        "{trailing_stop_market_error}"
    );
}

#[test]
fn stop_market_exit_submission_uses_trigger_price_without_book_liquidity() {
    let mut strategy = ready_to_trade_strategy();
    strategy.config.exit_order.order_type = OrderType::StopMarket;
    strategy.config.exit_order.time_in_force = TimeInForce::Gtc;
    strategy.config.exit_order.trigger_price = Some(0.40);
    strategy.config.exit_order.trigger_type = Some(TriggerType::LastPrice);
    strategy.config.exit_order.is_post_only = false;
    strategy.config.forced_exit_order = strategy.config.exit_order.clone();
    strategy.active.phase = SelectionPhase::Freeze;
    let instrument_id = selected_entry_instrument(&strategy);
    let mut book = configured_book_for_instrument(&mut strategy, instrument_id);
    book.bid_levels.clear();
    book.ask_levels.clear();
    book.best_bid = None;
    book.best_ask = None;
    book.liquidity_available = Some(500.0);
    let mut position = materialize_configured_position(
        &mut strategy,
        instrument_id,
        PositionId::from("P-STOP-EXIT"),
        Quantity::new(4.0, 2),
        0.450,
    );
    position.book = book;
    set_managed_position(
        &mut strategy,
        position,
        ManagedPositionOrigin::StrategyEntry,
    );

    let decision = strategy.exit_submission_decision_at(1_200);

    assert_eq!(decision.blocked_reason, None);
    assert_eq!(decision.forced_flat_reasons, vec![ForcedFlatReason::Freeze]);
    assert_eq!(decision.order_type, Some(OrderType::StopMarket));
    assert_eq!(decision.order_side, Some(OrderSide::Sell));
    assert_eq!(decision.price, Some(0.40));
    assert_eq!(decision.quantity, Some(Quantity::new(4.0, 2)));
}

#[test]
fn stop_market_exit_ev_uses_trigger_price_instead_of_live_book() {
    let mut strategy = ready_to_trade_strategy();
    strategy.config.exit_order.order_type = OrderType::StopMarket;
    strategy.config.exit_order.trigger_price = Some(0.40);
    strategy.config.exit_order.trigger_type = Some(TriggerType::LastPrice);
    strategy.config.exit_order.is_post_only = false;
    let instrument_id = selected_entry_instrument(&strategy);
    let position = materialize_configured_position(
        &mut strategy,
        instrument_id,
        PositionId::from("P-STOP-EV"),
        Quantity::new(4.0, 2),
        0.450,
    );
    set_managed_position(
        &mut strategy,
        position,
        ManagedPositionOrigin::StrategyEntry,
    );

    let decision = strategy.exit_submission_decision_at(1_200);
    let exit_ev_bps = decision
        .evaluation
        .exit_ev_bps
        .unwrap_or_else(|| panic!("triggered exit EV should be available: {decision:#?}"));
    let expected_exit_ev_bps = ((0.40 - 0.450) / 0.450) * BPS_DENOMINATOR;

    assert!((exit_ev_bps - expected_exit_ev_bps).abs() < 1e-9);
}

#[test]
fn task5_entry_order_plan_uses_configured_tif_and_side_specific_best_price() {
    let up = build_entry_order_plan(&EntryOrderPlanInputs {
        client_order_id: ClientOrderId::from("ENTRY-UP"),
        instrument_id: InstrumentId::from("condition-MKT-1-MKT-1-UP.POLYMARKET"),
        order_side: OrderSide::Buy,
        quantity: Quantity::non_zero(5.0, 0),
        price_precision: 2,
        time_in_force: TimeInForce::Fok,
        best_bid: 0.43,
        best_ask: 0.45,
    })
    .expect("up entry should have a valid plan");
    let down = build_entry_order_plan(&EntryOrderPlanInputs {
        client_order_id: ClientOrderId::from("ENTRY-DOWN"),
        instrument_id: InstrumentId::from("condition-MKT-1-MKT-1-DOWN.POLYMARKET"),
        order_side: OrderSide::Sell,
        quantity: Quantity::non_zero(5.0, 0),
        price_precision: 2,
        time_in_force: TimeInForce::Ioc,
        best_bid: 0.43,
        best_ask: 0.45,
    })
    .expect("down entry should have a valid plan");

    assert_eq!(up.order_side, OrderSide::Buy);
    assert_eq!(up.price, Price::new(0.45, 2));
    assert_eq!(up.time_in_force, TimeInForce::Fok);
    assert_eq!(down.order_side, OrderSide::Sell);
    assert_eq!(down.price, Price::new(0.43, 2));
    assert_eq!(down.time_in_force, TimeInForce::Ioc);
}

#[test]
fn expected_exit_submission_blocks_do_not_warn() {
    assert!(!should_warn_on_exit_submission_block(Some(
        EXIT_BLOCK_REASON_NO_OPEN_POSITION
    )));
    assert!(!should_warn_on_exit_submission_block(Some(
        EXIT_BLOCK_REASON_EXIT_ALREADY_PENDING
    )));
    assert!(!should_warn_on_exit_submission_block(Some(
        EXIT_BLOCK_REASON_ENTRY_ORDER_STILL_WORKING
    )));
    assert!(!should_warn_on_exit_submission_block(Some(
        EXIT_BLOCK_REASON_EXIT_HOLD
    )));
    assert!(!should_warn_on_exit_submission_block(Some(
        EXIT_BLOCK_REASON_POSITION_INTERVAL_UNKNOWN
    )));
    assert!(should_warn_on_exit_submission_block(Some(
        EXIT_BLOCK_REASON_EXIT_PRICE_MISSING
    )));
}

#[test]
fn task5_forced_flat_predicates_cover_current_strategy_visible_triggers() {
    let reasons = evaluate_forced_flat_predicates(&ForcedFlatInputs {
        frozen: true,
        metadata_matches_selection: false,
        last_reference_ts_ms: Some(1_000),
        now_ms: 3_000,
        stale_reference_after_ms: 1_500,
        liquidity_available: Some(50.0),
        min_liquidity_required: 100.0,
        fast_venue_incoherent: true,
    });

    assert_eq!(
        reasons,
        vec![
            ForcedFlatReason::Freeze,
            ForcedFlatReason::StaleReference,
            ForcedFlatReason::ThinBook,
            ForcedFlatReason::MetadataMismatch,
            ForcedFlatReason::FastVenueIncoherent,
        ]
    );
}

#[test]
fn quarantined_legacy_short_position_blocks_exit_submission() {
    let mut strategy = ready_to_trade_strategy();
    let instrument_id = selected_entry_instrument(&strategy);
    let mut tracked_book = OutcomeBookState::from_instrument_id(instrument_id);
    tracked_book.last_observed_instrument_id = Some(instrument_id);
    tracked_book.best_bid = Some(0.520);
    tracked_book.best_ask = Some(0.530);
    tracked_book.liquidity_available = Some(100.0);
    set_unsupported_observed(
        &mut strategy,
        OpenPositionState {
            lifecycle: BoltV3PositionMarketLifecycle::missing(),
            instrument_id,
            position_id: PositionId::from("P-LEGACY-SHORT-001"),
            entry_order_side: OrderSide::Sell,
            side: PositionSide::Short,
            quantity: Quantity::new(5.0, 2),
            avg_px_open: 0.480,
            book: tracked_book,
        },
        UnsupportedObservedReason::BootstrappedUnsupportedContract,
    );

    let decision = strategy.exit_submission_decision_at(2_000);

    assert_eq!(decision.evaluation.exit_decision, None);
    assert_eq!(decision.instrument_id, None);
    assert_eq!(decision.order_side, None);
    assert_eq!(decision.price, None);
    assert_eq!(decision.quantity, None);
    // A quarantined/unsupported position is not a managed open position, so the
    // exit evaluation blocks with the precise NoOpenPosition reason. The decision
    // trace surfaces that real reason rather than the generic ExitDecisionUnavailable
    // (which previously masked it via an unconditional clobber).
    assert_eq!(decision.blocked_reason, Some("no_open_position"));
}

#[test]
fn task6_exit_submission_decision_forced_flat_submits_for_open_up_position() {
    let mut strategy = ready_to_trade_strategy();
    strategy.active.phase = SelectionPhase::Freeze;
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
        position_id: PositionId::from("P-UP-001"),
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

    let decision = strategy.exit_submission_decision_at(1_200);

    assert_eq!(decision.order_side, Some(OrderSide::Sell));
    assert_eq!(
        decision.instrument_id,
        strategy.active.books.up.instrument_id
    );
    assert_eq!(decision.price, strategy.active.books.up.best_bid);
    assert_eq!(decision.quantity, Some(Quantity::new(10.0, 2)));
    assert_eq!(decision.forced_flat_reasons, vec![ForcedFlatReason::Freeze]);
}

#[test]
fn task6_exit_submission_decision_forced_flat_submits_for_open_down_position() {
    let mut strategy = ready_to_trade_strategy();
    strategy.active.phase = SelectionPhase::Freeze;
    let open_position = OpenPositionState {
        lifecycle: BoltV3PositionMarketLifecycle::from_entry_context(
            Some("MKT-1".to_string()),
            Some(OutcomeSide::Down),
            Some(3_100.0),
            Some(3_100.0),
            Some(301_000),
            Some(1_000),
            Some(300),
        ),
        instrument_id: strategy.active.books.down.instrument_id.unwrap(),
        position_id: PositionId::from("P-DOWN-001"),
        entry_order_side: OrderSide::Buy,
        side: PositionSide::Long,
        quantity: Quantity::new(12.0, 2),
        avg_px_open: 0.480,
        book: strategy.active.books.down.clone(),
    };
    set_managed_position(
        &mut strategy,
        open_position,
        ManagedPositionOrigin::StrategyEntry,
    );

    let decision = strategy.exit_submission_decision_at(1_200);

    assert_eq!(decision.order_side, Some(OrderSide::Sell));
    assert_eq!(
        decision.instrument_id,
        strategy.active.books.down.instrument_id
    );
    assert_eq!(decision.price, strategy.active.books.down.best_bid);
    assert_eq!(decision.quantity, Some(Quantity::new(12.0, 2)));
    assert_eq!(decision.forced_flat_reasons, vec![ForcedFlatReason::Freeze]);
}

#[test]
fn task6_exit_submission_decision_uses_live_hold_vs_exit_boundary() {
    let mut strategy = ready_to_trade_strategy();
    let mut exit_book = strategy.active.books.up.clone();
    exit_book.best_bid = Some(0.550);
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
        position_id: PositionId::from("P-UP-002"),
        entry_order_side: OrderSide::Buy,
        side: PositionSide::Long,
        quantity: Quantity::new(10.0, 2),
        avg_px_open: 0.450,
        book: exit_book,
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

    let decision = strategy.exit_submission_decision_at(1_200);

    assert!(decision.forced_flat_reasons.is_empty());
    assert_eq!(decision.order_side, Some(OrderSide::Sell));
    assert_eq!(
        decision.instrument_id,
        strategy.active.books.up.instrument_id
    );
    assert_eq!(decision.price, Some(0.550));
    assert_eq!(decision.quantity, Some(Quantity::new(10.0, 2)));
    assert_eq!(decision.blocked_reason, None);
}
