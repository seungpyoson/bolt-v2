use std::{cell::RefCell, rc::Rc};

use bolt_v2::{
    bolt_v3_maker_event_fence::{ClientOrderId as MakerClientOrderId, OrderIdentity},
    bolt_v3_maker_order_compile::{
        MakerCompiledOrderCommand, MakerOrderCompileBlockReason, MakerOrderCompileInput,
        compile_maker_order_intent,
    },
    bolt_v3_maker_order_plan::MakerOrderIntent,
    bolt_v3_order_intent::{NtOrderBuildInputs, NtOrderTemplate, build_nt_order},
    bolt_v3_quote_lifecycle::Leg,
};
use nautilus_common::{
    clock::{Clock, TestClock},
    factories::OrderFactory,
};
use nautilus_model::{
    enums::{OrderSide, OrderType, TimeInForce},
    identifiers::{InstrumentId, StrategyId, TraderId},
    orders::{Order, OrderAny},
    types::{Price, Quantity},
};

#[test]
fn submit_intent_compiles_to_post_only_limit_nt_order_inputs() {
    let intent = MakerOrderIntent::Submit {
        leg: Leg::Yes,
        instrument_id: instrument_id(),
        order_side: OrderSide::Buy,
        order_identity: identity("O-19700101-000000-001-101-1", 2),
        price: 0.42,
        quantity: 25.0,
    };

    let decision = compile_maker_order_intent(MakerOrderCompileInput {
        intent: &intent,
        submit_template: &maker_submit_template(),
        price_precision: 2,
        quantity_precision: 2,
    });

    let Some(MakerCompiledOrderCommand::Submit {
        leg,
        template,
        inputs,
        fallback_price,
    }) = decision.command
    else {
        panic!("expected submit command, got {decision:?}");
    };
    assert_eq!(decision.blocked_by, None);
    assert_eq!(leg, Leg::Yes);
    assert_eq!(fallback_price, Price::new(0.42, 2));
    assert_eq!(
        inputs,
        NtOrderBuildInputs {
            instrument_id: instrument_id(),
            order_side: OrderSide::Buy,
            quantity: Quantity::new(25.0, 2),
            price: Some(Price::new(0.42, 2)),
            client_order_id: nautilus_model::identifiers::ClientOrderId::from(
                "O-19700101-000000-001-101-1"
            ),
        }
    );

    let order = build_nt_order(&mut order_factory(), "maker_submit", &template, inputs)
        .expect("compiled maker submit should build through shared NT order path");
    let OrderAny::Limit(order) = order else {
        panic!("expected limit order");
    };
    assert_eq!(order.instrument_id(), instrument_id());
    assert_eq!(order.order_side(), OrderSide::Buy);
    assert_eq!(order.price(), Some(Price::new(0.42, 2)));
    assert_eq!(order.quantity(), Quantity::new(25.0, 2));
    assert_eq!(order.time_in_force(), TimeInForce::Gtc);
    assert!(order.is_post_only());
}

#[test]
fn unsupported_submit_template_is_blocked_before_build_inputs() {
    let intent = MakerOrderIntent::Submit {
        leg: Leg::Yes,
        instrument_id: instrument_id(),
        order_side: OrderSide::Buy,
        order_identity: identity("O-19700101-000000-001-102-1", 2),
        price: 0.42,
        quantity: 25.0,
    };
    let mut template = maker_submit_template();
    template.is_post_only = false;

    let decision = compile_maker_order_intent(MakerOrderCompileInput {
        intent: &intent,
        submit_template: &template,
        price_precision: 2,
        quantity_precision: 2,
    });

    assert_eq!(decision.command, None);
    assert_eq!(
        decision.blocked_by,
        Some(MakerOrderCompileBlockReason::UnsupportedSubmitTemplate)
    );
}

#[test]
fn cancel_intent_compiles_to_cancel_command_with_active_identity() {
    let intent = MakerOrderIntent::Cancel {
        leg: Leg::No,
        instrument_id: instrument_id(),
        order_identity: identity("O-19700101-000000-001-103-1", 4),
    };

    let decision = compile_maker_order_intent(MakerOrderCompileInput {
        intent: &intent,
        submit_template: &maker_submit_template(),
        price_precision: 2,
        quantity_precision: 2,
    });

    assert_eq!(
        decision.command,
        Some(MakerCompiledOrderCommand::Cancel {
            leg: Leg::No,
            instrument_id: instrument_id(),
            client_order_id: nautilus_model::identifiers::ClientOrderId::from(
                "O-19700101-000000-001-103-1"
            ),
        })
    );
    assert_eq!(decision.blocked_by, None);
}

#[test]
fn modify_intent_compiles_with_active_identity_and_new_price_quantity() {
    let intent = MakerOrderIntent::Modify {
        leg: Leg::Yes,
        instrument_id: instrument_id(),
        order_identity: identity("O-19700101-000000-001-104-1", 5),
        price: 0.45,
        quantity: 12.0,
    };

    let decision = compile_maker_order_intent(MakerOrderCompileInput {
        intent: &intent,
        submit_template: &maker_submit_template(),
        price_precision: 2,
        quantity_precision: 2,
    });

    assert_eq!(
        decision.command,
        Some(MakerCompiledOrderCommand::Modify {
            leg: Leg::Yes,
            instrument_id: instrument_id(),
            client_order_id: nautilus_model::identifiers::ClientOrderId::from(
                "O-19700101-000000-001-104-1"
            ),
            price: Price::new(0.45, 2),
            quantity: Quantity::new(12.0, 2),
        })
    );
    assert_eq!(decision.blocked_by, None);
}

fn maker_submit_template() -> NtOrderTemplate {
    NtOrderTemplate {
        order_type: OrderType::Limit,
        time_in_force: TimeInForce::Gtc,
        expire_time: None,
        trigger_price: None,
        activation_price: None,
        trigger_type: None,
        trigger_instrument_id: None,
        trailing_offset: None,
        trailing_offset_type: None,
        is_post_only: true,
        is_reduce_only: false,
        is_quote_quantity: false,
    }
}

fn order_factory() -> OrderFactory {
    let clock: Rc<RefCell<dyn Clock>> = Rc::new(RefCell::new(TestClock::new()));
    OrderFactory::new(
        TraderId::new("MAKER-001"),
        StrategyId::new("MAKERSTRAT-001"),
        None,
        None,
        clock,
        false,
        true,
    )
}

fn identity(value: &str, generation: u64) -> OrderIdentity {
    OrderIdentity::new(MakerClientOrderId::new(value.to_string()), generation)
}

fn instrument_id() -> InstrumentId {
    InstrumentId::from("condition-MATCH-YES.POLYMARKET")
}
