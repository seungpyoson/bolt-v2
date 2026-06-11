#![cfg(test)]

use std::sync::Arc;

use super::*;
use crate::bolt_v3_quote_lifecycle::{Leg, LegState};

#[test]
fn static_event_maker_runtime_dispatches_two_post_only_limit_submits_through_shared_pipeline() {
    let mut harness = ready_static_maker_harness();
    let strategy = &mut harness.strategy;

    let (risk_handler, risk_messages) =
        get_typed_into_message_saving_handler::<TradingCommand>(None);
    msgbus::register_trading_command_endpoint(
        MessagingSwitchboard::risk_engine_queue_execute(),
        risk_handler,
    );

    let step = strategy
        .drive_maker_quotes_at(1_200)
        .expect("maker runtime should dispatch through NT");

    assert_eq!(step.dispatched.len(), 2);
    let messages = risk_messages.get_messages();
    assert_eq!(messages.len(), 2);
    let active_instruments = [
        strategy
            .active
            .books
            .up
            .instrument_id
            .expect("active up instrument should be selected"),
        strategy
            .active
            .books
            .down
            .instrument_id
            .expect("active down instrument should be selected"),
    ];
    for command in messages {
        let TradingCommand::SubmitOrder(command) = command else {
            panic!("expected maker runtime to submit post-only limit orders");
        };
        assert!(active_instruments.contains(&command.instrument_id));
        assert_eq!(command.order_init.order_side, OrderSide::Buy);
        assert_eq!(command.order_init.order_type, OrderType::Limit);
        assert_eq!(command.order_init.time_in_force, TimeInForce::Gtc);
        assert!(command.order_init.price.is_some());
        assert!(command.order_init.post_only);
        assert!(!command.order_init.reduce_only);
        assert!(!command.order_init.quote_quantity);
        assert_eq!(command.order_init.expire_time, None);
    }
    assert_eq!(harness.submit_admission.admitted_order_count(), 2);
    let maker_intents = harness
        .evidence
        .events()
        .into_iter()
        .filter(|event| {
            matches!(
                event,
                RecordedDecisionEvidenceEvent::OrderIntent(intent)
                    if intent.intent_kind == BoltV3OrderIntentKind::MakerQuote
            )
        })
        .count();
    assert_eq!(maker_intents, 2);
}

#[test]
fn maker_accept_reports_rest_orders_and_prevent_duplicate_submit_on_next_quote_tick() {
    let mut harness = ready_static_maker_harness();
    let strategy = &mut harness.strategy;

    let (risk_handler, risk_messages) =
        get_typed_into_message_saving_handler::<TradingCommand>(None);
    msgbus::register_trading_command_endpoint(
        MessagingSwitchboard::risk_engine_queue_execute(),
        risk_handler,
    );

    strategy
        .drive_maker_quotes_at(1_200)
        .expect("initial maker quote dispatch should succeed");
    let submitted = submitted_orders(risk_messages.get_messages());
    assert_eq!(submitted.len(), 2);

    for order in submitted {
        strategy.on_order_accepted(order_accepted_event(
            order.client_order_id,
            order.instrument_id,
        ));
    }

    let maker = strategy.maker.as_ref().expect("maker state should exist");
    assert_eq!(maker.market.leg_state(Leg::Yes), LegState::Resting);
    assert_eq!(maker.market.leg_state(Leg::No), LegState::Resting);

    let step = strategy
        .drive_maker_quotes_at(1_250)
        .expect("resting maker quote tick should not duplicate submits");

    assert!(step.dispatched.is_empty());
    assert_eq!(risk_messages.get_messages().len(), 2);
}

#[test]
fn maker_fill_reports_update_inventory_and_only_terminalize_when_working_quantity_is_exhausted() {
    let mut harness = ready_static_maker_harness();
    let strategy = &mut harness.strategy;

    let (risk_handler, risk_messages) =
        get_typed_into_message_saving_handler::<TradingCommand>(None);
    msgbus::register_trading_command_endpoint(
        MessagingSwitchboard::risk_engine_queue_execute(),
        risk_handler,
    );

    strategy
        .drive_maker_quotes_at(1_200)
        .expect("initial maker quote dispatch should succeed");
    let submitted = submitted_orders(risk_messages.get_messages());
    let yes_order = submitted
        .into_iter()
        .find(|order| Some(order.instrument_id) == strategy.active.books.up.instrument_id)
        .expect("YES submit should be present");
    strategy.on_order_accepted(order_accepted_event(
        yes_order.client_order_id,
        yes_order.instrument_id,
    ));

    strategy
        .on_order_filled(&maker_order_filled_event(
            yes_order.client_order_id,
            yes_order.instrument_id,
            1.0,
        ))
        .expect("partial maker fill should be handled");

    let maker = strategy.maker.as_ref().expect("maker state should exist");
    assert_eq!(maker.market.leg_state(Leg::Yes), LegState::Resting);
    assert!((maker.inventory.net_position() - 1.0).abs() < 1e-9);

    strategy
        .on_order_filled(&maker_order_filled_event(
            yes_order.client_order_id,
            yes_order.instrument_id,
            1.0,
        ))
        .expect("terminal maker fill should be handled");

    let maker = strategy.maker.as_ref().expect("maker state should exist");
    assert_eq!(maker.market.leg_state(Leg::Yes), LegState::Idle);
    assert!(maker.yes_expected.expected().is_none());
    assert!((maker.inventory.net_position() - 2.0).abs() < 1e-9);
}

#[test]
fn maker_requote_cancel_confirmation_dispatches_replacement_submit_on_cancel_resubmit_venues() {
    let mut harness = ready_static_maker_harness();
    let strategy = &mut harness.strategy;

    let (risk_handler, risk_messages) =
        get_typed_into_message_saving_handler::<TradingCommand>(None);
    msgbus::register_trading_command_endpoint(
        MessagingSwitchboard::risk_engine_queue_execute(),
        risk_handler,
    );
    let (exec_handler, exec_messages) =
        get_typed_into_message_saving_handler::<TradingCommand>(None);
    msgbus::register_trading_command_endpoint(
        MessagingSwitchboard::exec_engine_queue_execute(),
        exec_handler,
    );

    strategy
        .drive_maker_quotes_at(1_200)
        .expect("initial maker quote dispatch should succeed");
    let submitted = submitted_orders(risk_messages.get_messages());
    assert_eq!(submitted.len(), 2);
    for order in submitted {
        strategy.on_order_accepted(order_accepted_event(
            order.client_order_id,
            order.instrument_id,
        ));
    }

    strategy.pricing.last_reference_current_price = Some(0.70);
    strategy.pricing.last_reference_current_price_ts_ms = Some(1_900);
    let requote_step = strategy
        .drive_maker_quotes_at(1_900)
        .expect("moved quote should dispatch cancels on Polymarket");
    assert_eq!(requote_step.dispatched.len(), 2);

    let canceled = canceled_orders(exec_messages.get_messages());
    assert_eq!(canceled.len(), 2);
    let yes_cancel = canceled
        .into_iter()
        .find(|order| Some(order.instrument_id) == strategy.active.books.up.instrument_id)
        .expect("YES cancel should be present");
    strategy
        .on_order_canceled(&maker_order_canceled_event(
            yes_cancel.client_order_id,
            yes_cancel.instrument_id,
            1_900,
        ))
        .expect("cancel confirmation should be handled");

    let risk_after_cancel = risk_messages.get_messages();
    let replacement_submits = submitted_orders(risk_after_cancel[2..].to_vec());
    assert_eq!(replacement_submits.len(), 1);
    assert_eq!(
        replacement_submits[0].instrument_id,
        yes_cancel.instrument_id
    );
    assert_ne!(
        replacement_submits[0].client_order_id,
        yes_cancel.client_order_id
    );
    let maker = strategy.maker.as_ref().expect("maker state should exist");
    assert_eq!(maker.market.leg_state(Leg::Yes), LegState::SubmitPending);
}

struct MakerRuntimeHarness {
    strategy: BinaryOracleEdgeTaker,
    evidence: Arc<RecordingSequencedDecisionEvidenceWriter>,
    submit_admission: Arc<crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState>,
}

fn ready_static_maker_harness() -> MakerRuntimeHarness {
    let evidence = Arc::new(RecordingSequencedDecisionEvidenceWriter::default());
    let submit_admission = Arc::new(
        crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new(evidence.clone()),
    );
    let mut strategy = ready_to_trade_strategy_with_decision_evidence_and_submit_admission(
        evidence.clone(),
        submit_admission.clone(),
    );
    register_test_strategy_with_active_instruments(&mut strategy);
    strategy.context = strategy.context.clone().with_venue_contract(Arc::new(
        crate::venue_contract::VenueContract::load_and_validate(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("contracts")
                .join("polymarket.toml"),
        )
        .expect("shipped polymarket contract should load"),
    ));
    strategy.config.rotating_market_family =
        crate::bolt_v3_market_families::static_binary_event::KEY.to_string();
    strategy.config.static_fair_probability_source = Some("reference_current_price".to_string());
    strategy.config.reference_current_price =
        Some(reference_price_block_with_max_source_age_ms(1_000));
    strategy.config.maker_quote = Some(BinaryOracleEdgeTakerMakerQuoteConfig {
        yes_quantity: 2.0,
        no_quantity: 2.0,
        collateral_budget: 10.0,
        informed_fraction: 0.05,
        microprice_weight: 0.0,
        inventory_skew_gain: 0.0,
        position_cap: 10.0,
        half_spread_floor: 0.01,
        max_half_spread: 0.20,
        epsilon: 0.001,
        reference_tau_secs: 3_600.0,
        time_widen_cap: 2.0,
        requote_threshold: 0.01,
        requote_action_cost: 1,
        requote_min_interval_ms: 0,
        order: BinaryOracleEdgeTakerOrderConfig {
            side: "buy".to_string(),
            position_side: "long".to_string(),
            order_type: OrderType::Limit,
            time_in_force: TimeInForce::Gtc,
            expire_time_unix_nanos: None,
            trigger_price: None,
            activation_price: None,
            trigger_type: None,
            trigger_instrument_id: None,
            trailing_offset: None,
            trailing_offset_type: None,
            is_post_only: true,
            is_reduce_only: false,
            is_quote_quantity: false,
        },
    });
    strategy.pricing.last_reference_current_price = Some(0.54);
    strategy.pricing.last_reference_current_price_ts_ms = Some(1_200);

    MakerRuntimeHarness {
        strategy,
        evidence,
        submit_admission,
    }
}

#[derive(Debug, Clone, Copy)]
struct SubmittedMakerOrder {
    client_order_id: ClientOrderId,
    instrument_id: InstrumentId,
}

#[derive(Debug, Clone, Copy)]
struct CanceledMakerOrder {
    client_order_id: ClientOrderId,
    instrument_id: InstrumentId,
}

fn submitted_orders(messages: Vec<TradingCommand>) -> Vec<SubmittedMakerOrder> {
    messages
        .into_iter()
        .filter_map(|command| {
            let TradingCommand::SubmitOrder(command) = command else {
                return None;
            };
            Some(SubmittedMakerOrder {
                client_order_id: command.client_order_id,
                instrument_id: command.instrument_id,
            })
        })
        .collect()
}

fn canceled_orders(messages: Vec<TradingCommand>) -> Vec<CanceledMakerOrder> {
    messages
        .into_iter()
        .filter_map(|command| {
            let TradingCommand::CancelOrder(command) = command else {
                return None;
            };
            Some(CanceledMakerOrder {
                client_order_id: command.client_order_id,
                instrument_id: command.instrument_id,
            })
        })
        .collect()
}

fn order_accepted_event(
    client_order_id: ClientOrderId,
    instrument_id: InstrumentId,
) -> nautilus_model::events::OrderAccepted {
    nautilus_model::events::OrderAccepted::new(
        TraderId::from("TRADER-001"),
        StrategyId::from("BINARYORACLEEDGETAKER-001"),
        instrument_id,
        client_order_id,
        nautilus_model::identifiers::VenueOrderId::from("V-ORDER-001"),
        nautilus_model::identifiers::AccountId::from("TEST-ACCOUNT"),
        nautilus_core::UUID4::new(),
        UnixNanos::from(1_000_u64),
        UnixNanos::from(1_000_u64),
        false,
    )
}

fn maker_order_filled_event(
    client_order_id: ClientOrderId,
    instrument_id: InstrumentId,
    quantity: f64,
) -> nautilus_model::events::OrderFilled {
    nautilus_model::events::OrderFilled::new(
        TraderId::from("TRADER-001"),
        StrategyId::from("BINARYORACLEEDGETAKER-001"),
        instrument_id,
        client_order_id,
        nautilus_model::identifiers::VenueOrderId::from("V-ORDER-001"),
        nautilus_model::identifiers::AccountId::from("TEST-ACCOUNT"),
        nautilus_model::identifiers::TradeId::from("TRADE-001"),
        OrderSide::Buy,
        OrderType::Limit,
        Quantity::new(quantity, 2),
        Price::new(0.450, 3),
        nautilus_model::types::Currency::USDC(),
        nautilus_model::enums::LiquiditySide::Maker,
        nautilus_core::UUID4::new(),
        UnixNanos::from(1_000_u64),
        UnixNanos::from(1_000_u64),
        false,
        None,
        Some(nautilus_model::types::Money::new(
            0.0,
            nautilus_model::types::Currency::USDC(),
        )),
    )
}

fn maker_order_canceled_event(
    client_order_id: ClientOrderId,
    instrument_id: InstrumentId,
    ts_ms: u64,
) -> nautilus_model::events::OrderCanceled {
    nautilus_model::events::OrderCanceled::new(
        TraderId::from("TRADER-001"),
        StrategyId::from("BINARYORACLEEDGETAKER-001"),
        instrument_id,
        client_order_id,
        nautilus_core::UUID4::new(),
        UnixNanos::from(ts_ms * NANOS_PER_MILLI_U64),
        UnixNanos::from(ts_ms * NANOS_PER_MILLI_U64),
        false,
        Some(nautilus_model::identifiers::VenueOrderId::from(
            "V-ORDER-001",
        )),
        Some(nautilus_model::identifiers::AccountId::from("TEST-ACCOUNT")),
    )
}

fn reference_price_block_with_max_source_age_ms(
    max_source_age_ms: u64,
) -> crate::bolt_v3_config::ReferencePriceBlock {
    crate::bolt_v3_config::ReferencePriceBlock {
        asset: "WORLD_CUP_STATIC_FAIR".to_string(),
        source_order: Vec::new(),
        min_valid_sources: 0,
        selection_policy:
            crate::bolt_v3_config::ReferencePriceSelectionPolicy::FirstValidPerInterval,
        max_source_age_ms,
        max_source_drift_bps: 0,
        drift_policy: crate::bolt_v3_config::ReferencePriceDriftPolicy::Observe,
        stale_policy: crate::bolt_v3_config::ReferencePriceStalePolicy::Block,
        sources: std::collections::BTreeMap::new(),
    }
}
