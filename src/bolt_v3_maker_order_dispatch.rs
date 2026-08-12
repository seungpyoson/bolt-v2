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
    bolt_v3_maker_order_compile::MakerCompiledOrderCommand,
    bolt_v3_order_execution::BoltV3RestingSubmitTransactionOutcome,
    bolt_v3_order_intent::build_nt_order, bolt_v3_quote_lifecycle::Leg,
};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MakerOrderDispatchInput<'a> {
    pub command: &'a MakerCompiledOrderCommand,
    pub submit_order_prefix: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MakerOrderDispatchOutcome {
    SubmitAttempt {
        leg: Leg,
        instrument_id: InstrumentId,
        prepared_client_order_id: ClientOrderId,
        price: Price,
        quantity: Quantity,
        transaction: BoltV3RestingSubmitTransactionOutcome,
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

#[cfg(any(test, feature = "test-current-evidence-inspection"))]
impl MakerOrderDispatchOutcome {
    #[must_use]
    pub fn submitted_for_test(
        leg: Leg,
        instrument_id: InstrumentId,
        client_order_id: ClientOrderId,
        price: Price,
        quantity: Quantity,
    ) -> Self {
        Self::SubmitAttempt {
            leg,
            instrument_id,
            prepared_client_order_id: client_order_id,
            price,
            quantity,
            transaction: BoltV3RestingSubmitTransactionOutcome::submitted_with_linkage_for_test(
                instrument_id,
                OrderSide::Buy,
                price,
                quantity,
                client_order_id,
            ),
        }
    }

    #[must_use]
    pub fn policy_skipped_for_test(
        leg: Leg,
        instrument_id: InstrumentId,
        client_order_id: ClientOrderId,
        price: Price,
        quantity: Quantity,
    ) -> Self {
        Self::SubmitAttempt {
            leg,
            instrument_id,
            prepared_client_order_id: client_order_id,
            price,
            quantity,
            transaction: BoltV3RestingSubmitTransactionOutcome::policy_skipped_for_test(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MakerOrderCommandFailureKind {
    Lifecycle,
    Build,
    SubmitPreparation,
    Cancel,
    CancelAll,
    Modify,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MakerOrderCommandFailure {
    kind: MakerOrderCommandFailureKind,
    diagnostic: String,
}

impl MakerOrderCommandFailure {
    pub(crate) fn new(kind: MakerOrderCommandFailureKind, error: impl std::fmt::Display) -> Self {
        Self {
            kind,
            diagnostic: error.to_string(),
        }
    }

    #[must_use]
    pub const fn kind(&self) -> MakerOrderCommandFailureKind {
        self.kind
    }

    #[must_use]
    pub fn diagnostic(&self) -> &str {
        &self.diagnostic
    }

    #[cfg(any(test, feature = "test-current-evidence-inspection"))]
    #[must_use]
    pub fn for_test(kind: MakerOrderCommandFailureKind, diagnostic: impl Into<String>) -> Self {
        Self {
            kind,
            diagnostic: diagnostic.into(),
        }
    }
}

impl std::fmt::Display for MakerOrderCommandFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "maker command {:?} failure: {}",
            self.kind, self.diagnostic
        )
    }
}

impl std::error::Error for MakerOrderCommandFailure {}

pub trait MakerOrderCommandSink {
    type PreparedSubmit;

    fn order_factory(&mut self) -> RefMut<'_, OrderFactory>;

    fn prepare_maker_order(&mut self, order: OrderAny) -> Result<Self::PreparedSubmit>;

    fn submit_maker_order(
        &mut self,
        prepared: Self::PreparedSubmit,
    ) -> BoltV3RestingSubmitTransactionOutcome;

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
) -> std::result::Result<MakerOrderDispatchOutcome, MakerOrderCommandFailure> {
    match input.command {
        MakerCompiledOrderCommand::Submit {
            leg,
            template,
            inputs,
            fallback_price,
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
                )
                .map_err(|error| {
                    MakerOrderCommandFailure::new(MakerOrderCommandFailureKind::Build, error)
                })?
            };
            let prepared_client_order_id = order.client_order_id();
            let instrument_id = order.instrument_id();
            let price = order.price().unwrap_or(*fallback_price);
            let quantity = order.quantity();
            let prepared = sink.prepare_maker_order(order).map_err(|error| {
                MakerOrderCommandFailure::new(
                    MakerOrderCommandFailureKind::SubmitPreparation,
                    error,
                )
            })?;
            let transaction = sink.submit_maker_order(prepared);
            Ok(MakerOrderDispatchOutcome::SubmitAttempt {
                leg: *leg,
                instrument_id,
                prepared_client_order_id,
                price,
                quantity,
                transaction,
            })
        }
        MakerCompiledOrderCommand::Cancel {
            leg,
            instrument_id,
            client_order_id,
        } => {
            sink.cancel_maker_order(*leg, *instrument_id, *client_order_id)
                .map_err(|error| {
                    MakerOrderCommandFailure::new(MakerOrderCommandFailureKind::Cancel, error)
                })?;
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
            sink.cancel_all_maker_orders(*leg, *instrument_id, *order_side)
                .map_err(|error| {
                    MakerOrderCommandFailure::new(MakerOrderCommandFailureKind::CancelAll, error)
                })?;
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
            sink.modify_maker_order(*leg, *instrument_id, *client_order_id, *price, *quantity)
                .map_err(|error| {
                    MakerOrderCommandFailure::new(MakerOrderCommandFailureKind::Modify, error)
                })?;
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
