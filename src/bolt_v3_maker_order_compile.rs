//! Shared maker order-intent compiler for NautilusTrader command inputs.
//!
//! The maker planner emits venue-agnostic intents. This module keeps the runtime
//! bridge shared by compiling those intents into the existing NT order template
//! and build-input surface without owning strategy decisions.

use crate::{
    bolt_v3_maker_order_plan::MakerOrderIntent,
    bolt_v3_numeric::is_positive_finite,
    bolt_v3_order_intent::{NtOrderBuildInputs, NtOrderTemplate},
    bolt_v3_quote_lifecycle::Leg,
};
use nautilus_model::{
    enums::{OrderType, TimeInForce},
    identifiers::{ClientOrderId, InstrumentId},
    types::{Price, Quantity},
};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MakerOrderCompileInput<'a> {
    pub intent: &'a MakerOrderIntent,
    pub submit_template: &'a NtOrderTemplate,
    pub price_precision: u8,
    pub quantity_precision: u8,
}

#[derive(Debug, Clone, PartialEq)]
pub enum MakerCompiledOrderCommand {
    Submit {
        leg: Leg,
        template: Box<NtOrderTemplate>,
        inputs: NtOrderBuildInputs,
        fallback_price: Price,
    },
    Cancel {
        leg: Leg,
        instrument_id: InstrumentId,
        client_order_id: ClientOrderId,
    },
    Modify {
        leg: Leg,
        instrument_id: InstrumentId,
        client_order_id: ClientOrderId,
        price: Price,
        quantity: Quantity,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MakerOrderCompileBlockReason {
    UnsupportedSubmitTemplate,
    InvalidSubmitPrice,
    InvalidSubmitQuantity,
    InvalidModifyPrice,
    InvalidModifyQuantity,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MakerOrderCompileDecision {
    pub command: Option<MakerCompiledOrderCommand>,
    pub blocked_by: Option<MakerOrderCompileBlockReason>,
}

pub fn compile_maker_order_intent(input: MakerOrderCompileInput<'_>) -> MakerOrderCompileDecision {
    match input.intent {
        MakerOrderIntent::Submit {
            leg,
            instrument_id,
            order_side,
            order_identity,
            price,
            quantity,
            ..
        } => compile_submit(
            input,
            *leg,
            *instrument_id,
            *order_side,
            order_identity,
            *price,
            *quantity,
        ),
        MakerOrderIntent::Cancel {
            leg,
            instrument_id,
            order_identity,
        } => compiled(MakerCompiledOrderCommand::Cancel {
            leg: *leg,
            instrument_id: *instrument_id,
            client_order_id: nt_client_order_id(order_identity),
        }),
        MakerOrderIntent::Modify {
            leg,
            instrument_id,
            order_identity,
            price,
            quantity,
        } => compile_modify(
            input,
            *leg,
            *instrument_id,
            order_identity,
            *price,
            *quantity,
        ),
    }
}

fn compile_submit(
    input: MakerOrderCompileInput<'_>,
    leg: Leg,
    instrument_id: InstrumentId,
    order_side: nautilus_model::enums::OrderSide,
    order_identity: &crate::bolt_v3_maker_event_fence::OrderIdentity,
    price: f64,
    quantity: f64,
) -> MakerOrderCompileDecision {
    if !maker_submit_template_is_supported(input.submit_template) {
        return blocked(MakerOrderCompileBlockReason::UnsupportedSubmitTemplate);
    }
    if !is_positive_finite(price) {
        return blocked(MakerOrderCompileBlockReason::InvalidSubmitPrice);
    }
    if !is_positive_finite(quantity) {
        return blocked(MakerOrderCompileBlockReason::InvalidSubmitQuantity);
    }

    let price = Price::new(price, input.price_precision);
    let quantity = Quantity::new(quantity, input.quantity_precision);
    compiled(MakerCompiledOrderCommand::Submit {
        leg,
        template: Box::new(input.submit_template.clone()),
        inputs: NtOrderBuildInputs {
            instrument_id,
            order_side,
            quantity,
            price: Some(price),
            client_order_id: nt_client_order_id(order_identity),
        },
        fallback_price: price,
    })
}

fn compile_modify(
    input: MakerOrderCompileInput<'_>,
    leg: Leg,
    instrument_id: InstrumentId,
    order_identity: &crate::bolt_v3_maker_event_fence::OrderIdentity,
    price: f64,
    quantity: f64,
) -> MakerOrderCompileDecision {
    if !is_positive_finite(price) {
        return blocked(MakerOrderCompileBlockReason::InvalidModifyPrice);
    }
    if !is_positive_finite(quantity) {
        return blocked(MakerOrderCompileBlockReason::InvalidModifyQuantity);
    }

    compiled(MakerCompiledOrderCommand::Modify {
        leg,
        instrument_id,
        client_order_id: nt_client_order_id(order_identity),
        price: Price::new(price, input.price_precision),
        quantity: Quantity::new(quantity, input.quantity_precision),
    })
}

pub fn maker_submit_template_is_supported(template: &NtOrderTemplate) -> bool {
    template.order_type == OrderType::Limit
        && template.time_in_force == TimeInForce::Gtc
        && template.expire_time.is_none()
        && template.trigger_price.is_none()
        && template.activation_price.is_none()
        && template.trigger_type.is_none()
        && template.trigger_instrument_id.is_none()
        && template.trailing_offset.is_none()
        && template.trailing_offset_type.is_none()
        && template.is_post_only
        && !template.is_reduce_only
        && !template.is_quote_quantity
}

fn nt_client_order_id(
    order_identity: &crate::bolt_v3_maker_event_fence::OrderIdentity,
) -> ClientOrderId {
    ClientOrderId::from(order_identity.client_order_id().as_str())
}

fn compiled(command: MakerCompiledOrderCommand) -> MakerOrderCompileDecision {
    MakerOrderCompileDecision {
        command: Some(command),
        blocked_by: None,
    }
}

fn blocked(reason: MakerOrderCompileBlockReason) -> MakerOrderCompileDecision {
    MakerOrderCompileDecision {
        command: None,
        blocked_by: Some(reason),
    }
}
