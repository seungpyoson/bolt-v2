//! Shared maker order-command dispatcher.
//!
//! The maker compile layer produces typed commands. This module binds those
//! commands to the existing NT order-construction path and a caller-provided
//! runtime sink, so strategies do not own maker submit/cancel/modify mechanics.

use std::cell::RefMut;

use anyhow::Result;
use nautilus_common::factories::OrderFactory;
use nautilus_model::{
    enums::OrderSide,
    identifiers::{ClientOrderId, InstrumentId},
    orders::{Order, OrderAny},
    types::{Price, Quantity},
};

use crate::{
    bolt_v3_maker_order_compile::MakerCompiledOrderCommand, bolt_v3_order_intent::build_nt_order,
    bolt_v3_quote_lifecycle::Leg,
};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MakerOrderDispatchInput<'a> {
    pub command: &'a MakerCompiledOrderCommand,
    pub submit_order_prefix: &'a str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MakerOrderDispatchOutcome {
    Submitted {
        leg: Leg,
        instrument_id: InstrumentId,
        client_order_id: ClientOrderId,
        price: Price,
        quantity: Quantity,
    },
    Canceled {
        leg: Leg,
        instrument_id: InstrumentId,
        client_order_id: ClientOrderId,
    },
    CanceledAll {
        leg: Option<Leg>,
        instrument_id: InstrumentId,
        order_side: Option<OrderSide>,
    },
    Modified {
        leg: Leg,
        instrument_id: InstrumentId,
        client_order_id: ClientOrderId,
        price: Price,
        quantity: Quantity,
    },
}

pub trait MakerOrderCommandSink {
    fn order_factory(&mut self) -> RefMut<'_, OrderFactory>;

    fn submit_maker_order(&mut self, order: OrderAny, gross_expected_value: f64) -> Result<()>;

    fn cancel_maker_order(
        &mut self,
        leg: Leg,
        instrument_id: InstrumentId,
        client_order_id: ClientOrderId,
    ) -> Result<()>;

    fn cancel_all_maker_orders(
        &mut self,
        leg: Option<Leg>,
        instrument_id: InstrumentId,
        order_side: Option<OrderSide>,
    ) -> Result<()>;

    fn modify_maker_order(
        &mut self,
        leg: Leg,
        instrument_id: InstrumentId,
        client_order_id: ClientOrderId,
        price: Price,
        quantity: Quantity,
    ) -> Result<()>;
}

pub fn dispatch_maker_order_command(
    input: MakerOrderDispatchInput<'_>,
    sink: &mut impl MakerOrderCommandSink,
) -> Result<MakerOrderDispatchOutcome> {
    match input.command {
        MakerCompiledOrderCommand::Submit {
            leg,
            template,
            inputs,
            fallback_price,
            gross_expected_value,
        } => {
            let order = {
                // `order_factory()` now yields a `RefMut` guard (NT moved the strategy
                // `OrderFactory` behind `Rc<RefCell<_>>`). Scope it so the borrow of `sink`
                // is released before the `submit_maker_order` call below.
                let mut order_factory = sink.order_factory();
                build_nt_order(
                    &mut order_factory,
                    input.submit_order_prefix,
                    template,
                    *inputs,
                )?
            };
            let client_order_id = order.client_order_id();
            let instrument_id = order.instrument_id();
            let price = order.price().unwrap_or(*fallback_price);
            let quantity = order.quantity();
            sink.submit_maker_order(order, *gross_expected_value)?;
            Ok(MakerOrderDispatchOutcome::Submitted {
                leg: *leg,
                instrument_id,
                client_order_id,
                price,
                quantity,
            })
        }
        MakerCompiledOrderCommand::Cancel {
            leg,
            instrument_id,
            client_order_id,
        } => {
            sink.cancel_maker_order(*leg, *instrument_id, *client_order_id)?;
            Ok(MakerOrderDispatchOutcome::Canceled {
                leg: *leg,
                instrument_id: *instrument_id,
                client_order_id: *client_order_id,
            })
        }
        MakerCompiledOrderCommand::CancelAll {
            leg,
            instrument_id,
            order_side,
        } => {
            sink.cancel_all_maker_orders(*leg, *instrument_id, *order_side)?;
            Ok(MakerOrderDispatchOutcome::CanceledAll {
                leg: *leg,
                instrument_id: *instrument_id,
                order_side: *order_side,
            })
        }
        MakerCompiledOrderCommand::Modify {
            leg,
            instrument_id,
            client_order_id,
            price,
            quantity,
        } => {
            sink.modify_maker_order(*leg, *instrument_id, *client_order_id, *price, *quantity)?;
            Ok(MakerOrderDispatchOutcome::Modified {
                leg: *leg,
                instrument_id: *instrument_id,
                client_order_id: *client_order_id,
                price: *price,
                quantity: *quantity,
            })
        }
    }
}
