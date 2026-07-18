use std::{cell::RefCell, rc::Rc};

use bolt_v2::bolt_v3_order_intent::{
    MarketQuoteBuyQuantityError, NtOrderBuildInputs, NtOrderTemplate, NtOrderTemplateConfig,
    build_nt_order, check_nt_order_template_config, make_market_quote_buy_quantity,
    normalize_base_order_quantity, validate_nt_order_template,
};
use nautilus_common::{
    clock::{Clock, TestClock},
    factories::OrderFactory,
};
use nautilus_core::UnixNanos;
use nautilus_model::{
    enums::{AssetClass, OrderSide, OrderType, TimeInForce, TrailingOffsetType, TriggerType},
    identifiers::{ClientOrderId, InstrumentId, StrategyId, Symbol, TraderId, Venue},
    instruments::{BinaryOption, InstrumentAny},
    orders::{Order, OrderAny},
    types::{Currency, Price, Quantity},
};
use rust_decimal::Decimal;
use ustr::Ustr;

fn binary_option() -> InstrumentAny {
    InstrumentAny::BinaryOption(BinaryOption::new(
        InstrumentId::from("GENERIC.TEST"),
        Symbol::from("generic"),
        AssetClass::Alternative,
        Currency::USD(),
        UnixNanos::from(1_u64),
        UnixNanos::from(2_u64),
        2,
        2,
        Price::from("0.01"),
        Quantity::from("0.01"),
        Some(Ustr::from("YES")),
        None,
        None,
        Some(Quantity::from("0.01")),
        None,
        None,
        Some(Price::from("1.00")),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        UnixNanos::from(1_u64),
        UnixNanos::from(1_u64),
    ))
}

#[test]
fn shared_venue_quantity_wrappers_apply_provider_and_instrument_rounding() {
    let instrument = binary_option();

    let normalized = normalize_base_order_quantity(
        Venue::from("POLYMARKET"),
        &instrument,
        Quantity::new(2.641, 3),
    )
    .expect("positive Polymarket quantity should normalize");

    assert_eq!(normalized, Quantity::new(2.64, 2));
    assert_eq!(
        normalize_base_order_quantity(
            Venue::from("POLYMARKET"),
            &instrument,
            Quantity::new(0.001, 3),
        ),
        None,
        "provider underflow must fail closed"
    );
}

#[test]
fn shared_market_quote_buy_quantity_fails_closed_for_unknown_venue_and_below_minimum() {
    let instrument = binary_option();

    assert_eq!(
        make_market_quote_buy_quantity(
            Venue::from("POLYMARKET"),
            &instrument,
            Decimal::new(1234, 3),
        ),
        Ok(Quantity::new(1.23, 2)),
        "modeled minimum and instrument precision should be applied together"
    );

    assert_eq!(
        make_market_quote_buy_quantity(Venue::from("HYPERLIQUID"), &instrument, Decimal::ONE),
        Err(MarketQuoteBuyQuantityError::MinimumUnmodeled)
    );
    assert_eq!(
        make_market_quote_buy_quantity(Venue::from("POLYMARKET"), &instrument, Decimal::new(99, 2),),
        Err(MarketQuoteBuyQuantityError::BelowMinimum)
    );
}

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

fn base_config(order_type: OrderType) -> NtOrderTemplateConfig {
    NtOrderTemplateConfig {
        order_type,
        time_in_force: TimeInForce::Gtc,
        expire_time_unix_nanos: None,
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
        price: Some(Price::new(10.0, 2)),
        client_order_id: ClientOrderId::from("O-19700101-000000-001-001-1"),
    }
}

fn base_inputs_without_price(order_side: OrderSide) -> NtOrderBuildInputs {
    NtOrderBuildInputs {
        price: None,
        ..base_inputs(order_side)
    }
}

fn limit_price() -> Price {
    base_inputs(OrderSide::Buy)
        .price
        .expect("base inputs should include a limit price")
}

fn nonzero_expire_time() -> UnixNanos {
    let price = limit_price();
    let raw = u64::try_from(price.raw.unsigned_abs())
        .expect("generic base price raw value should fit a test timestamp");
    UnixNanos::from(raw)
}

fn zero_price() -> Price {
    let price = limit_price();
    Price::from_raw(Default::default(), price.precision)
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

fn zero_trailing_offset() -> Decimal {
    positive_trailing_offset() - positive_trailing_offset()
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

fn assert_validate_error_contains(
    template: &NtOrderTemplate,
    order_side: OrderSide,
    expected: &str,
) {
    let error = validate_nt_order_template("generic_order", template, &base_inputs(order_side))
        .expect_err("invalid generic template should fail direct validation");
    assert!(error.to_string().contains(expected), "{error}");
}

fn assert_config_error_contains(order_type: OrderType, expected: &str) {
    let errors = check_nt_order_template_config(
        "generic_context",
        "generic_order",
        &base_config(order_type),
    );
    assert!(
        errors.iter().any(|error| error.contains(expected)),
        "{errors:#?}"
    );
}

#[test]
fn shared_nt_order_template_builds_post_only_limit_without_submission_context() {
    let mut factory = generic_order_factory();
    let mut template = base_template(OrderType::Limit);
    template.is_post_only = true;
    template.expire_time = Some(nonzero_expire_time());

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
    assert_eq!(order.expire_time(), Some(nonzero_expire_time()));
    assert!(order.is_post_only());
}

#[test]
fn shared_nt_order_template_builds_sell_limit_if_touched_without_position_policy() {
    let mut factory = generic_order_factory();
    let mut template = base_template(OrderType::LimitIfTouched);
    template.trigger_price = Some(trigger_price_above_limit());

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
    assert_eq!(order.price(), Some(limit_price()));
    assert_eq!(order.trigger_price(), Some(trigger_price_above_limit()));
}

#[test]
fn trailing_stop_trigger_only_keeps_activation_unset() {
    let mut factory = generic_order_factory();
    let mut template = base_template(OrderType::TrailingStopMarket);
    let trigger_price = trigger_price_below_limit();
    template.trigger_price = Some(trigger_price);
    template.trailing_offset = Some(positive_trailing_offset());

    let order = build_nt_order(
        &mut factory,
        "generic_order",
        &template,
        base_inputs(OrderSide::Sell),
    )
    .expect("trigger-only trailing stop should build through NT factory");

    let OrderAny::TrailingStopMarket(order) = order else {
        panic!("expected NT TrailingStopMarket order");
    };
    assert_eq!(order.trigger_price(), Some(trigger_price));
    assert_eq!(order.activation_price(), None);
}

#[test]
fn trailing_stop_activation_only_keeps_trigger_unset() {
    let mut factory = generic_order_factory();
    let mut template = base_template(OrderType::TrailingStopMarket);
    let activation_price = trigger_price_below_limit();
    template.activation_price = Some(activation_price);
    template.trailing_offset = Some(positive_trailing_offset());

    let order = build_nt_order(
        &mut factory,
        "generic_order",
        &template,
        base_inputs(OrderSide::Sell),
    )
    .expect("activation-only trailing stop should build through NT factory");

    let OrderAny::TrailingStopMarket(order) = order else {
        panic!("expected NT TrailingStopMarket order");
    };
    assert_eq!(order.trigger_price(), None);
    assert_eq!(order.activation_price(), Some(activation_price));
}

#[test]
fn shared_nt_order_template_price_is_required_only_for_limit_price_factories() {
    let mut factory = generic_order_factory();

    for order_type in [
        OrderType::Market,
        OrderType::StopMarket,
        OrderType::MarketIfTouched,
        OrderType::TrailingStopMarket,
    ] {
        let template = valid_template_for_direct_validation(order_type);
        build_nt_order(
            &mut factory,
            "generic_order",
            &template,
            base_inputs_without_price(OrderSide::Buy),
        )
        .expect("market-like NT factory order should not require a limit price input");
    }

    for order_type in [
        OrderType::Limit,
        OrderType::StopLimit,
        OrderType::LimitIfTouched,
    ] {
        let template = valid_template_for_direct_validation(order_type);
        let error = build_nt_order(
            &mut factory,
            "generic_order",
            &template,
            base_inputs_without_price(OrderSide::Buy),
        )
        .expect_err("limit-price NT factory order should reject missing price input");
        assert!(error.to_string().contains("price is required"), "{error}");
    }
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
fn shared_nt_order_template_preserves_trigger_instrument_id_for_triggered_factories() {
    let mut factory = generic_order_factory();
    let trigger_instrument_id = base_inputs(OrderSide::Buy).instrument_id;

    for order_type in [
        OrderType::StopMarket,
        OrderType::StopLimit,
        OrderType::MarketIfTouched,
        OrderType::LimitIfTouched,
        OrderType::TrailingStopMarket,
    ] {
        let mut template = valid_template_for_direct_validation(order_type);
        template.trigger_instrument_id = Some(trigger_instrument_id);
        let order = build_nt_order(
            &mut factory,
            "generic_order",
            &template,
            base_inputs(OrderSide::Buy),
        )
        .expect("triggered order should preserve NT trigger_instrument_id");
        assert_eq!(order.trigger_instrument_id(), Some(trigger_instrument_id));
    }
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
fn shared_nt_order_template_rejects_remaining_direct_caller_validation_invariants() {
    let mut factory = generic_order_factory();

    let mut market_expiry = base_template(OrderType::Market);
    market_expiry.expire_time = Some(nonzero_expire_time());
    assert_build_error_contains(
        &mut factory,
        &market_expiry,
        OrderSide::Buy,
        "expire_time is not supported for Market orders",
    );

    for order_type in [
        OrderType::StopMarket,
        OrderType::StopLimit,
        OrderType::MarketIfTouched,
        OrderType::LimitIfTouched,
    ] {
        let template = base_template(order_type);
        assert_build_error_contains(
            &mut factory,
            &template,
            OrderSide::Buy,
            "trigger_price is required for triggered orders",
        );
    }

    for order_type in [
        OrderType::Limit,
        OrderType::Market,
        OrderType::StopMarket,
        OrderType::StopLimit,
        OrderType::MarketIfTouched,
        OrderType::LimitIfTouched,
    ] {
        let mut activation_template = valid_template_for_direct_validation(order_type);
        activation_template.activation_price = Some(limit_price());
        assert_build_error_contains(
            &mut factory,
            &activation_template,
            OrderSide::Buy,
            "activation_price is only supported for TrailingStopMarket orders",
        );

        let mut trailing_offset_template = valid_template_for_direct_validation(order_type);
        trailing_offset_template.trailing_offset = Some(positive_trailing_offset());
        assert_build_error_contains(
            &mut factory,
            &trailing_offset_template,
            OrderSide::Buy,
            "trailing_offset is only supported for TrailingStopMarket orders",
        );

        let mut trailing_offset_type_template = valid_template_for_direct_validation(order_type);
        trailing_offset_type_template.trailing_offset_type = Some(TrailingOffsetType::Price);
        assert_build_error_contains(
            &mut factory,
            &trailing_offset_type_template,
            OrderSide::Buy,
            "trailing_offset_type is only supported for TrailingStopMarket orders",
        );
    }

    for order_type in [OrderType::Limit, OrderType::Market] {
        let mut trigger_type_template = base_template(order_type);
        trigger_type_template.trigger_type = Some(TriggerType::Default);
        assert_build_error_contains(
            &mut factory,
            &trigger_type_template,
            OrderSide::Buy,
            "trigger_type is only supported for triggered orders",
        );

        let mut trigger_instrument_template = base_template(order_type);
        trigger_instrument_template.trigger_instrument_id =
            Some(base_inputs(OrderSide::Buy).instrument_id);
        assert_build_error_contains(
            &mut factory,
            &trigger_instrument_template,
            OrderSide::Buy,
            "trigger_instrument_id is only supported for triggered orders",
        );
    }

    let trailing_without_trigger = base_template(OrderType::TrailingStopMarket);
    assert_build_error_contains(
        &mut factory,
        &trailing_without_trigger,
        OrderSide::Sell,
        "trigger_price or generic_order_activation_price is required",
    );

    let mut trailing_without_offset = base_template(OrderType::TrailingStopMarket);
    trailing_without_offset.activation_price = Some(limit_price());
    assert_build_error_contains(
        &mut factory,
        &trailing_without_offset,
        OrderSide::Sell,
        "trailing_offset is required for TrailingStopMarket orders",
    );

    let mut trailing_zero_offset = base_template(OrderType::TrailingStopMarket);
    trailing_zero_offset.activation_price = Some(limit_price());
    trailing_zero_offset.trailing_offset = Some(zero_trailing_offset());
    assert_build_error_contains(
        &mut factory,
        &trailing_zero_offset,
        OrderSide::Sell,
        "trailing_offset must be positive",
    );
}

#[test]
fn shared_nt_order_template_rejects_order_arm_post_only_invariants_before_nt_factory() {
    let mut factory = generic_order_factory();

    for (order_type, expected) in [
        (
            OrderType::Market,
            "is_post_only must be false for market orders",
        ),
        (
            OrderType::StopMarket,
            "is_post_only must be false for StopMarket orders",
        ),
        (
            OrderType::MarketIfTouched,
            "is_post_only must be false for MarketIfTouched orders",
        ),
    ] {
        let mut template = valid_template_for_direct_validation(order_type);
        template.is_post_only = true;
        assert_build_error_contains(&mut factory, &template, OrderSide::Buy, expected);
    }
}

#[test]
fn shared_nt_order_template_preserves_configured_trigger_and_trailing_types() {
    let mut factory = generic_order_factory();

    for order_type in [
        OrderType::StopMarket,
        OrderType::StopLimit,
        OrderType::MarketIfTouched,
        OrderType::LimitIfTouched,
        OrderType::TrailingStopMarket,
    ] {
        let mut template = valid_template_for_direct_validation(order_type);
        template.trigger_type = Some(TriggerType::LastPrice);
        let order = build_nt_order(
            &mut factory,
            "generic_order",
            &template,
            base_inputs(OrderSide::Buy),
        )
        .expect("configured trigger type should build through NT factory");
        assert_eq!(order.trigger_type(), Some(TriggerType::LastPrice));
    }

    let mut trailing_template = valid_template_for_direct_validation(OrderType::TrailingStopMarket);
    trailing_template.trailing_offset_type = Some(TrailingOffsetType::Price);
    let trailing_order = build_nt_order(
        &mut factory,
        "generic_order",
        &trailing_template,
        base_inputs(OrderSide::Buy),
    )
    .expect("configured trailing offset type should build through NT factory");
    assert_eq!(
        trailing_order.trailing_offset_type(),
        Some(TrailingOffsetType::Price)
    );
}

#[test]
fn direct_nt_order_template_validation_rejects_market_like_post_only_orders() {
    for (order_type, expected) in [
        (
            OrderType::Market,
            "is_post_only must be false for market orders",
        ),
        (
            OrderType::StopMarket,
            "is_post_only must be false for StopMarket orders",
        ),
        (
            OrderType::MarketIfTouched,
            "is_post_only must be false for MarketIfTouched orders",
        ),
    ] {
        let mut template = valid_template_for_direct_validation(order_type);
        template.is_post_only = true;
        assert_validate_error_contains(&template, OrderSide::Buy, expected);
    }
}

#[test]
fn shared_nt_order_template_rejects_unsupported_factory_gap_variants() {
    let mut factory = generic_order_factory();

    for order_type in [OrderType::MarketToLimit, OrderType::TrailingStopLimit] {
        assert_build_error_contains(
            &mut factory,
            &base_template(order_type),
            OrderSide::Buy,
            "is not exposed by the pinned NT single-order OrderFactory",
        );
    }
}

#[test]
fn shared_nt_order_template_config_rejects_unsupported_factory_gap_variants() {
    for order_type in [OrderType::MarketToLimit, OrderType::TrailingStopLimit] {
        assert_config_error_contains(
            order_type,
            "is not exposed by the pinned NT single-order OrderFactory",
        );
    }
}

#[test]
fn direct_nt_order_template_validation_rejects_unsupported_factory_gap_variants() {
    for order_type in [OrderType::MarketToLimit, OrderType::TrailingStopLimit] {
        assert_validate_error_contains(
            &base_template(order_type),
            OrderSide::Buy,
            "is not exposed by the pinned NT single-order OrderFactory",
        );
    }
}

#[test]
fn shared_nt_order_template_directly_rejects_trailing_stop_market_post_only_once() {
    let mut template = valid_template_for_direct_validation(OrderType::TrailingStopMarket);
    template.is_post_only = true;
    assert_validate_error_contains(
        &template,
        OrderSide::Buy,
        "is_post_only must be false for TrailingStopMarket orders",
    );

    let source = std::fs::read_to_string("src/bolt_v3_order_intent.rs")
        .expect("shared order-intent module should exist");
    assert_eq!(
        source
            .matches("is_post_only must be false for TrailingStopMarket orders")
            .count(),
        1,
        "TrailingStopMarket post-only rejection should live in direct validation only"
    );
}

fn source_contains_forbidden_pattern(source: &str, forbidden: &str) -> bool {
    if matches!(forbidden, "Entry" | "Exit") {
        return source
            .split(|character: char| !(character.is_ascii_alphanumeric() || character == '_'))
            .any(|token| token == forbidden);
    }
    source.contains(forbidden)
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
        "parameters.",
        "Entry",
        "Exit",
    ] {
        assert!(
            !source_contains_forbidden_pattern(&source, forbidden),
            "shared order-intent module must not contain `{forbidden}`"
        );
    }
}
