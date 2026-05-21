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
use rust_decimal::Decimal;

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

fn limit_price() -> Price {
    base_inputs(OrderSide::Buy).price
}

fn zero_price() -> Price {
    let price = limit_price();
    Price::from_raw(price.raw - price.raw, price.precision)
}

fn negative_price() -> Price {
    let price = limit_price();
    Price::from_raw(-price.raw, price.precision)
}

fn trigger_price_below_limit() -> Price {
    let price = limit_price();
    Price::from_raw(price.raw - price.raw.signum(), price.precision)
}

fn trigger_price_above_limit() -> Price {
    let price = limit_price();
    Price::from_raw(price.raw + price.raw.signum(), price.precision)
}

fn positive_trailing_offset() -> Decimal {
    limit_price().as_decimal()
}

fn valid_template_for_direct_validation(order_type: OrderType) -> NtOrderTemplate {
    let mut template = base_template(order_type);
    match order_type {
        OrderType::StopMarket | OrderType::StopLimit | OrderType::MarketIfTouched => {
            template.trigger_price = Some(trigger_price_below_limit());
        }
        OrderType::LimitIfTouched => {
            template.trigger_price = Some(trigger_price_below_limit());
        }
        OrderType::TrailingStopMarket => {
            template.activation_price = Some(trigger_price_below_limit());
            template.trailing_offset = Some(positive_trailing_offset());
        }
        _ => {}
    }
    template
}

fn assert_build_error_contains(
    factory: &mut OrderFactory,
    template: &NtOrderTemplate,
    order_side: OrderSide,
    expected: &str,
) {
    let error = build_nt_order(factory, "generic_order", template, base_inputs(order_side))
        .expect_err("invalid generic template should fail before NT factory construction");
    assert!(error.to_string().contains(expected), "{error}");
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

    for order_type in [
        OrderType::StopMarket,
        OrderType::StopLimit,
        OrderType::MarketIfTouched,
        OrderType::LimitIfTouched,
    ] {
        let mut template = base_template(order_type);
        template.trigger_price = Some(zero_price());
        assert_build_error_contains(
            &mut factory,
            &template,
            OrderSide::Buy,
            "trigger_price must be positive",
        );
    }

    let mut trailing_stop_trigger = base_template(OrderType::TrailingStopMarket);
    trailing_stop_trigger.trigger_price = Some(zero_price());
    trailing_stop_trigger.trailing_offset = Some(positive_trailing_offset());
    assert_build_error_contains(
        &mut factory,
        &trailing_stop_trigger,
        OrderSide::Sell,
        "trigger_price must be positive",
    );

    let mut trailing_stop = base_template(OrderType::TrailingStopMarket);
    trailing_stop.activation_price = Some(zero_price());
    trailing_stop.trailing_offset = Some(positive_trailing_offset());
    assert_build_error_contains(
        &mut factory,
        &trailing_stop,
        OrderSide::Sell,
        "activation_price must be positive",
    );
}

#[test]
fn shared_nt_order_template_rejects_negative_trigger_inputs_before_nt_factory() {
    let mut factory = generic_order_factory();

    for order_type in [
        OrderType::StopMarket,
        OrderType::StopLimit,
        OrderType::MarketIfTouched,
        OrderType::LimitIfTouched,
    ] {
        let mut template = base_template(order_type);
        template.trigger_price = Some(negative_price());
        assert_build_error_contains(
            &mut factory,
            &template,
            OrderSide::Buy,
            "trigger_price must be positive",
        );
    }

    let mut trailing_stop_trigger = base_template(OrderType::TrailingStopMarket);
    trailing_stop_trigger.trigger_price = Some(negative_price());
    trailing_stop_trigger.trailing_offset = Some(positive_trailing_offset());
    assert_build_error_contains(
        &mut factory,
        &trailing_stop_trigger,
        OrderSide::Sell,
        "trigger_price must be positive",
    );

    let mut trailing_stop_activation = base_template(OrderType::TrailingStopMarket);
    trailing_stop_activation.activation_price = Some(negative_price());
    trailing_stop_activation.trailing_offset = Some(positive_trailing_offset());
    assert_build_error_contains(
        &mut factory,
        &trailing_stop_activation,
        OrderSide::Sell,
        "activation_price must be positive",
    );
}

#[test]
fn shared_nt_order_template_rejects_direct_caller_nt_model_invariants_before_nt_factory() {
    let mut factory = generic_order_factory();

    for order_type in [
        OrderType::Limit,
        OrderType::StopMarket,
        OrderType::StopLimit,
        OrderType::MarketIfTouched,
        OrderType::LimitIfTouched,
        OrderType::TrailingStopMarket,
    ] {
        let mut template = valid_template_for_direct_validation(order_type);
        template.time_in_force = TimeInForce::Gtd;
        assert_build_error_contains(
            &mut factory,
            &template,
            OrderSide::Buy,
            "expire_time is required",
        );
    }

    let mut market_gtd = base_template(OrderType::Market);
    market_gtd.time_in_force = TimeInForce::Gtd;
    assert_build_error_contains(
        &mut factory,
        &market_gtd,
        OrderSide::Buy,
        "GTD not supported for Market orders",
    );

    let mut trailing_stop_post_only =
        valid_template_for_direct_validation(OrderType::TrailingStopMarket);
    trailing_stop_post_only.is_post_only = true;
    assert_build_error_contains(
        &mut factory,
        &trailing_stop_post_only,
        OrderSide::Sell,
        "is_post_only must be false",
    );

    let mut buy_lit = base_template(OrderType::LimitIfTouched);
    buy_lit.trigger_price = Some(trigger_price_above_limit());
    assert_build_error_contains(
        &mut factory,
        &buy_lit,
        OrderSide::Buy,
        "trigger_price must be <= order price",
    );

    let mut sell_lit = base_template(OrderType::LimitIfTouched);
    sell_lit.trigger_price = Some(trigger_price_below_limit());
    assert_build_error_contains(
        &mut factory,
        &sell_lit,
        OrderSide::Sell,
        "trigger_price must be >= order price",
    );

    for order_type in [OrderType::Limit, OrderType::Market] {
        let mut template = base_template(order_type);
        template.trigger_price = Some(trigger_price_below_limit());
        assert_build_error_contains(
            &mut factory,
            &template,
            OrderSide::Buy,
            "trigger_price is only supported for triggered orders",
        );
    }
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
