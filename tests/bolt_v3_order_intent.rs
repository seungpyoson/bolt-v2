use std::{cell::RefCell, rc::Rc};

use bolt_v2::bolt_v3_order_intent::{NtOrderBuildInputs, NtOrderTemplate, build_nt_order};
use nautilus_common::{
    clock::{Clock, TestClock},
    factories::OrderFactory,
};
use nautilus_core::UnixNanos;
use nautilus_model::{
    enums::{OrderSide, OrderType, TimeInForce},
    identifiers::{ClientOrderId, InstrumentId, StrategyId, TraderId},
    orders::{Order, OrderAny},
    types::{Price, Quantity},
};

fn generic_order_factory() -> OrderFactory {
    let clock: Rc<RefCell<dyn Clock>> = Rc::new(RefCell::new(TestClock::new()));
    OrderFactory::new(
        TraderId::new("GENERIC-001"),
        StrategyId::new("GENERICORDER-001"),
        None,
        None,
        clock,
        false,
        true,
    )
}

fn base_template(order_type: OrderType) -> NtOrderTemplate {
    NtOrderTemplate {
        order_type,
        time_in_force: TimeInForce::Gtc,
        expire_time: None,
        trigger_price: None,
        activation_price: None,
        trigger_type: None,
        trigger_instrument_id: None,
        trailing_offset: None,
        trailing_offset_type: None,
        is_post_only: false,
        is_reduce_only: false,
        is_quote_quantity: false,
    }
}

fn base_inputs(order_side: OrderSide) -> NtOrderBuildInputs {
    NtOrderBuildInputs {
        instrument_id: InstrumentId::from("GENERIC.TEST"),
        order_side,
        quantity: Quantity::new(1.0, 2),
        price: Price::new(10.0, 2),
        client_order_id: ClientOrderId::from("O-19700101-000000-001-001-1"),
    }
}

#[test]
fn shared_nt_order_template_builds_post_only_limit_without_submission_context() {
    let mut factory = generic_order_factory();
    let mut template = base_template(OrderType::Limit);
    template.is_post_only = true;
    template.expire_time = Some(UnixNanos::from(1_000_000_000_u64));

    let order = build_nt_order(
        &mut factory,
        "generic_order",
        &template,
        base_inputs(OrderSide::Buy),
    )
    .expect("generic order template should build through NT OrderFactory");

    let OrderAny::Limit(order) = order else {
        panic!("expected NT Limit order");
    };
    assert_eq!(order.order_side(), OrderSide::Buy);
    assert_eq!(order.time_in_force(), TimeInForce::Gtc);
    assert_eq!(
        order.expire_time(),
        Some(UnixNanos::from(1_000_000_000_u64))
    );
    assert!(order.is_post_only());
}

#[test]
fn shared_nt_order_template_builds_sell_limit_if_touched_without_position_policy() {
    let mut factory = generic_order_factory();
    let mut template = base_template(OrderType::LimitIfTouched);
    template.trigger_price = Some(Price::new(12.0, 2));

    let order = build_nt_order(
        &mut factory,
        "generic_order",
        &template,
        base_inputs(OrderSide::Sell),
    )
    .expect("generic order template should not encode long-only strategy policy");

    let OrderAny::LimitIfTouched(order) = order else {
        panic!("expected NT LimitIfTouched order");
    };
    assert_eq!(order.order_side(), OrderSide::Sell);
    assert_eq!(order.price(), Some(Price::new(10.0, 2)));
    assert_eq!(order.trigger_price(), Some(Price::new(12.0, 2)));
}

#[test]
fn shared_nt_order_template_rejects_non_positive_trigger_inputs_before_nt_factory() {
    let mut factory = generic_order_factory();

    let mut stop_market = base_template(OrderType::StopMarket);
    stop_market.trigger_price = Some(Price::new(0.0, 2));
    let error = build_nt_order(
        &mut factory,
        "generic_order",
        &stop_market,
        base_inputs(OrderSide::Buy),
    )
    .expect_err("zero trigger price must fail before NT factory construction");
    assert!(
        error.to_string().contains("trigger_price must be positive"),
        "{error}"
    );

    let mut trailing_stop = base_template(OrderType::TrailingStopMarket);
    trailing_stop.activation_price = Some(Price::new(0.0, 2));
    trailing_stop.trailing_offset = Some(rust_decimal::Decimal::new(1, 1));
    let error = build_nt_order(
        &mut factory,
        "generic_order",
        &trailing_stop,
        base_inputs(OrderSide::Sell),
    )
    .expect_err("zero activation price must fail before NT factory construction");
    assert!(
        error
            .to_string()
            .contains("activation_price must be positive"),
        "{error}"
    );
}

#[test]
fn shared_nt_order_template_source_has_no_strategy_venue_market_or_submit_coupling() {
    let source = std::fs::read_to_string("src/bolt_v3_order_intent.rs")
        .expect("shared order-intent module should exist");
    for forbidden in [
        "binary_oracle",
        "polymarket",
        "market_family",
        "strategy_archetype",
        "StrategyCore",
        "StrategyId",
        "PositionSide",
        "SubmitContext",
        "submit_order",
        "submit_admission",
        "BoltV3OrderIntentEvidence",
        "Entry",
        "Exit",
    ] {
        assert!(
            !source.contains(forbidden),
            "shared order-intent module must not contain `{forbidden}`"
        );
    }
}
