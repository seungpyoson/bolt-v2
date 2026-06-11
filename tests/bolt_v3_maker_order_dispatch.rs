use std::{cell::RefCell, rc::Rc};

use anyhow::Result;
use bolt_v2::{
    bolt_v3_maker_event_fence::{ClientOrderId as MakerClientOrderId, OrderIdentity},
    bolt_v3_maker_order_compile::{
        MakerCompiledOrderCommand, MakerOrderCompileInput, compile_maker_order_intent,
    },
    bolt_v3_maker_order_dispatch::{
        MakerOrderCommandSink, MakerOrderDispatchInput, MakerOrderDispatchOutcome,
        dispatch_maker_order_command,
    },
    bolt_v3_maker_order_plan::MakerOrderIntent,
    bolt_v3_order_intent::{NtOrderBuildInputs, NtOrderTemplate},
    bolt_v3_quote_lifecycle::Leg,
};
use nautilus_common::{
    clock::{Clock, TestClock},
    factories::OrderFactory,
};
use nautilus_model::{
    enums::{OrderSide, OrderType, TimeInForce},
    identifiers::{ClientOrderId, InstrumentId, StrategyId, TraderId},
    orders::{Order, OrderAny},
    types::{Price, Quantity},
};

#[test]
fn submit_command_builds_nt_order_before_calling_submit_sink() {
    let command = compiled_submit_command();
    let mut sink = RecordingMakerOrderSink::new();

    let outcome = dispatch_maker_order_command(
        MakerOrderDispatchInput {
            command: &command,
            submit_order_prefix: "maker_submit",
        },
        &mut sink,
    )
    .expect("compiled submit command should dispatch");

    assert_eq!(
        outcome,
        MakerOrderDispatchOutcome::Submitted {
            leg: Leg::Yes,
            instrument_id: instrument_id(),
            client_order_id: ClientOrderId::from("O-19700101-000000-001-201-1"),
            price: Price::new(0.41, 2),
            quantity: Quantity::new(18.0, 2),
        }
    );
    assert!(sink.cancels.is_empty());
    assert!(sink.modifies.is_empty());
    assert_eq!(sink.submits.len(), 1);
    let OrderAny::Limit(order) = &sink.submits[0] else {
        panic!("maker submit must build an NT limit order");
    };
    assert_eq!(order.instrument_id(), instrument_id());
    assert_eq!(
        order.client_order_id(),
        ClientOrderId::from("O-19700101-000000-001-201-1")
    );
    assert_eq!(order.order_side(), OrderSide::Buy);
    assert_eq!(order.price(), Some(Price::new(0.41, 2)));
    assert_eq!(order.quantity(), Quantity::new(18.0, 2));
    assert_eq!(order.time_in_force(), TimeInForce::Gtc);
    assert!(order.is_post_only());
}

#[test]
fn cancel_command_routes_identity_to_cancel_sink_without_building_order() {
    let command = MakerCompiledOrderCommand::Cancel {
        leg: Leg::No,
        instrument_id: instrument_id(),
        client_order_id: ClientOrderId::from("O-19700101-000000-001-202-1"),
    };
    let mut sink = RecordingMakerOrderSink::new();

    let outcome = dispatch_maker_order_command(
        MakerOrderDispatchInput {
            command: &command,
            submit_order_prefix: "maker_submit",
        },
        &mut sink,
    )
    .expect("cancel command should dispatch");

    assert_eq!(
        outcome,
        MakerOrderDispatchOutcome::Canceled {
            leg: Leg::No,
            instrument_id: instrument_id(),
            client_order_id: ClientOrderId::from("O-19700101-000000-001-202-1"),
        }
    );
    assert!(sink.submits.is_empty());
    assert!(sink.modifies.is_empty());
    assert_eq!(
        sink.cancels,
        vec![CancelCall {
            leg: Leg::No,
            instrument_id: instrument_id(),
            client_order_id: ClientOrderId::from("O-19700101-000000-001-202-1"),
        }]
    );
}

#[test]
fn modify_command_routes_identity_price_and_quantity_to_modify_sink() {
    let command = MakerCompiledOrderCommand::Modify {
        leg: Leg::Yes,
        instrument_id: instrument_id(),
        client_order_id: ClientOrderId::from("O-19700101-000000-001-203-1"),
        price: Price::new(0.44, 2),
        quantity: Quantity::new(9.0, 2),
    };
    let mut sink = RecordingMakerOrderSink::new();

    let outcome = dispatch_maker_order_command(
        MakerOrderDispatchInput {
            command: &command,
            submit_order_prefix: "maker_submit",
        },
        &mut sink,
    )
    .expect("modify command should dispatch");

    assert_eq!(
        outcome,
        MakerOrderDispatchOutcome::Modified {
            leg: Leg::Yes,
            instrument_id: instrument_id(),
            client_order_id: ClientOrderId::from("O-19700101-000000-001-203-1"),
            price: Price::new(0.44, 2),
            quantity: Quantity::new(9.0, 2),
        }
    );
    assert!(sink.submits.is_empty());
    assert!(sink.cancels.is_empty());
    assert_eq!(
        sink.modifies,
        vec![ModifyCall {
            leg: Leg::Yes,
            instrument_id: instrument_id(),
            client_order_id: ClientOrderId::from("O-19700101-000000-001-203-1"),
            price: Price::new(0.44, 2),
            quantity: Quantity::new(9.0, 2),
        }]
    );
}

#[test]
fn submit_build_failure_does_not_call_submit_sink() {
    let command = MakerCompiledOrderCommand::Submit {
        leg: Leg::Yes,
        template: Box::new(maker_submit_template()),
        inputs: NtOrderBuildInputs {
            instrument_id: instrument_id(),
            order_side: OrderSide::Buy,
            quantity: Quantity::new(18.0, 2),
            price: None,
            client_order_id: ClientOrderId::from("O-19700101-000000-001-204-1"),
        },
        fallback_price: Price::new(0.41, 2),
    };
    let mut sink = RecordingMakerOrderSink::new();

    let error = dispatch_maker_order_command(
        MakerOrderDispatchInput {
            command: &command,
            submit_order_prefix: "maker_submit",
        },
        &mut sink,
    )
    .expect_err("invalid submit command should fail before sink call");

    assert!(
        error.to_string().contains("price is required"),
        "unexpected error: {error:#}"
    );
    assert!(sink.submits.is_empty());
    assert!(sink.cancels.is_empty());
    assert!(sink.modifies.is_empty());
}

fn compiled_submit_command() -> MakerCompiledOrderCommand {
    let intent = MakerOrderIntent::Submit {
        leg: Leg::Yes,
        instrument_id: instrument_id(),
        order_side: OrderSide::Buy,
        order_identity: identity("O-19700101-000000-001-201-1", 2),
        price: 0.41,
        quantity: 18.0,
    };
    compile_maker_order_intent(MakerOrderCompileInput {
        intent: &intent,
        submit_template: &maker_submit_template(),
        price_precision: 2,
        quantity_precision: 2,
    })
    .command
    .expect("submit intent should compile")
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CancelCall {
    leg: Leg,
    instrument_id: InstrumentId,
    client_order_id: ClientOrderId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ModifyCall {
    leg: Leg,
    instrument_id: InstrumentId,
    client_order_id: ClientOrderId,
    price: Price,
    quantity: Quantity,
}

struct RecordingMakerOrderSink {
    order_factory: OrderFactory,
    submits: Vec<OrderAny>,
    cancels: Vec<CancelCall>,
    modifies: Vec<ModifyCall>,
}

impl RecordingMakerOrderSink {
    fn new() -> Self {
        Self {
            order_factory: order_factory(),
            submits: Vec::new(),
            cancels: Vec::new(),
            modifies: Vec::new(),
        }
    }
}

impl MakerOrderCommandSink for RecordingMakerOrderSink {
    fn order_factory(&mut self) -> &mut OrderFactory {
        &mut self.order_factory
    }

    fn submit_maker_order(&mut self, order: OrderAny) -> Result<()> {
        self.submits.push(order);
        Ok(())
    }

    fn cancel_maker_order(
        &mut self,
        leg: Leg,
        instrument_id: InstrumentId,
        client_order_id: ClientOrderId,
    ) -> Result<()> {
        self.cancels.push(CancelCall {
            leg,
            instrument_id,
            client_order_id,
        });
        Ok(())
    }

    fn modify_maker_order(
        &mut self,
        leg: Leg,
        instrument_id: InstrumentId,
        client_order_id: ClientOrderId,
        price: Price,
        quantity: Quantity,
    ) -> Result<()> {
        self.modifies.push(ModifyCall {
            leg,
            instrument_id,
            client_order_id,
            price,
            quantity,
        });
        Ok(())
    }
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
