use anyhow::Result;
use nautilus_common::factories::OrderFactory;
use nautilus_core::UnixNanos;
use nautilus_model::{
    enums::{OrderSide, OrderType, TimeInForce, TrailingOffsetType, TriggerType},
    identifiers::{ClientOrderId, InstrumentId, Venue},
    instruments::{Instrument, InstrumentAny},
    orders::OrderAny,
    types::{Price, Quantity},
};
use rust_decimal::{
    Decimal,
    prelude::{FromPrimitive, ToPrimitive},
};

use crate::bolt_v3_providers::{
    market_quote_buy_min_notional_for_execution_venue,
    normalize_base_order_quantity_for_execution_venue,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarketQuoteBuyQuantityError {
    MinimumUnmodeled,
    BelowMinimum,
    QuantityInvalid,
}

pub fn normalize_base_order_quantity(
    venue: Venue,
    instrument: &InstrumentAny,
    quantity: Quantity,
) -> Option<Quantity> {
    let quantity = Decimal::from_f64(quantity.as_f64())?;
    let normalized = normalize_base_order_quantity_for_execution_venue(venue, quantity)?;
    instrument
        .try_make_qty(normalized.to_f64()?, Some(true))
        .ok()
}

pub fn make_market_quote_buy_quantity(
    venue: Venue,
    instrument: &InstrumentAny,
    quote_notional: Decimal,
) -> std::result::Result<Quantity, MarketQuoteBuyQuantityError> {
    let minimum = market_quote_buy_min_notional_for_execution_venue(venue)
        .ok_or(MarketQuoteBuyQuantityError::MinimumUnmodeled)?;
    if quote_notional < minimum {
        return Err(MarketQuoteBuyQuantityError::BelowMinimum);
    }
    instrument
        .try_make_qty(
            quote_notional
                .to_f64()
                .ok_or(MarketQuoteBuyQuantityError::QuantityInvalid)?,
            Some(true),
        )
        .map_err(|_| MarketQuoteBuyQuantityError::QuantityInvalid)
}

#[derive(Debug, Clone, PartialEq)]
pub struct NtOrderTemplate {
    pub order_type: OrderType,
    pub time_in_force: TimeInForce,
    pub expire_time: Option<UnixNanos>,
    pub trigger_price: Option<Price>,
    pub activation_price: Option<Price>,
    pub trigger_type: Option<TriggerType>,
    pub trigger_instrument_id: Option<InstrumentId>,
    pub trailing_offset: Option<Decimal>,
    pub trailing_offset_type: Option<TrailingOffsetType>,
    pub is_post_only: bool,
    pub is_reduce_only: bool,
    pub is_quote_quantity: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NtOrderBuildInputs {
    pub instrument_id: InstrumentId,
    pub order_side: OrderSide,
    pub quantity: Quantity,
    pub price: Option<Price>,
    pub client_order_id: ClientOrderId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NtOrderTemplateConfig {
    pub order_type: OrderType,
    pub time_in_force: TimeInForce,
    pub expire_time_unix_nanos: Option<u64>,
    pub trigger_price: Option<Decimal>,
    pub activation_price: Option<Decimal>,
    pub trigger_type: Option<TriggerType>,
    pub trigger_instrument_id: Option<InstrumentId>,
    pub trailing_offset: Option<Decimal>,
    pub trailing_offset_type: Option<TrailingOffsetType>,
    pub is_post_only: bool,
    pub is_reduce_only: bool,
    pub is_quote_quantity: bool,
}

pub fn check_nt_order_template_config(
    context: &str,
    field_path: &str,
    order: &NtOrderTemplateConfig,
) -> Vec<String> {
    match order.order_type {
        OrderType::Market => check_market_order_template(context, field_path, order),
        OrderType::Limit => check_limit_order_template(context, field_path, order),
        OrderType::StopMarket | OrderType::MarketIfTouched => {
            check_triggered_market_order_template(context, field_path, order)
        }
        OrderType::StopLimit | OrderType::LimitIfTouched => {
            check_triggered_limit_order_template(context, field_path, order)
        }
        OrderType::TrailingStopMarket => {
            check_trailing_stop_market_combination(context, field_path, order)
        }
        _ => vec![format!(
            "{context}: {field_path}.order_type `{}` is not exposed by the pinned NT single-order OrderFactory",
            order.order_type
        )],
    }
}

fn check_market_order_template(
    context: &str,
    field_path: &str,
    order: &NtOrderTemplateConfig,
) -> Vec<String> {
    let mut errors = Vec::new();
    if order.time_in_force == TimeInForce::Gtd {
        errors.push(format!(
            "{context}: {field_path}.time_in_force=gtd is not supported for order_type=market"
        ));
    }
    if order.expire_time_unix_nanos.is_some() {
        errors.push(format!(
            "{context}: {field_path}.expire_time_unix_nanos is not supported for order_type=market"
        ));
    }
    if order.is_post_only {
        errors.push(format!(
            "{context}: {field_path}.is_post_only must be false for order_type=market"
        ));
    }
    errors.extend(check_no_trigger_or_trailing_fields(
        context, field_path, order,
    ));
    errors
}

fn check_limit_order_template(
    context: &str,
    field_path: &str,
    order: &NtOrderTemplateConfig,
) -> Vec<String> {
    let mut errors = Vec::new();
    if order.time_in_force == TimeInForce::Gtd
        && order.expire_time_unix_nanos.is_none_or(|value| value == 0)
    {
        errors.push(format!(
            "{context}: {field_path}.expire_time_unix_nanos is required for GTD limit orders"
        ));
    }
    errors.extend(check_no_trigger_or_trailing_fields(
        context, field_path, order,
    ));
    errors
}

fn check_triggered_market_order_template(
    context: &str,
    field_path: &str,
    order: &NtOrderTemplateConfig,
) -> Vec<String> {
    let mut errors = check_triggered_order_template(context, field_path, order);
    if order.is_post_only {
        errors.push(format!(
            "{context}: {field_path}.is_post_only must be false for order_type={}",
            order.order_type
        ));
    }
    errors
}

fn check_triggered_limit_order_template(
    context: &str,
    field_path: &str,
    order: &NtOrderTemplateConfig,
) -> Vec<String> {
    check_triggered_order_template(context, field_path, order)
}

fn check_triggered_order_template(
    context: &str,
    field_path: &str,
    order: &NtOrderTemplateConfig,
) -> Vec<String> {
    let mut errors = Vec::new();
    if order
        .trigger_price
        .is_none_or(|value| value <= Decimal::ZERO)
    {
        errors.push(format!(
            "{context}: {field_path}.trigger_price must be positive for order_type={}",
            order.order_type
        ));
    }
    if order.time_in_force == TimeInForce::Gtd
        && order.expire_time_unix_nanos.is_none_or(|value| value == 0)
    {
        errors.push(format!(
            "{context}: {field_path}.expire_time_unix_nanos is required for GTD {} orders",
            order.order_type
        ));
    }
    errors.extend(check_no_trailing_stop_market_fields(
        context, field_path, order,
    ));
    errors
}

fn check_no_trigger_or_trailing_fields(
    context: &str,
    field_path: &str,
    order: &NtOrderTemplateConfig,
) -> Vec<String> {
    let mut errors = check_no_trailing_stop_market_fields(context, field_path, order);
    if order.trigger_price.is_some() {
        errors.push(format!(
            "{context}: {field_path}.trigger_price is only supported for triggered orders"
        ));
    }
    if order.trigger_type.is_some() {
        errors.push(format!(
            "{context}: {field_path}.trigger_type is only supported for triggered orders"
        ));
    }
    if order.trigger_instrument_id.is_some() {
        errors.push(format!(
            "{context}: {field_path}.trigger_instrument_id is only supported for triggered orders"
        ));
    }
    errors
}

fn check_no_trailing_stop_market_fields(
    context: &str,
    field_path: &str,
    order: &NtOrderTemplateConfig,
) -> Vec<String> {
    let mut errors = Vec::new();
    if order.activation_price.is_some() {
        errors.push(format!(
            "{context}: {field_path}.activation_price is only supported for order_type=trailing_stop_market"
        ));
    }
    if order.trailing_offset.is_some() {
        errors.push(format!(
            "{context}: {field_path}.trailing_offset is only supported for order_type=trailing_stop_market"
        ));
    }
    if order.trailing_offset_type.is_some() {
        errors.push(format!(
            "{context}: {field_path}.trailing_offset_type is only supported for order_type=trailing_stop_market"
        ));
    }
    errors
}

fn check_trailing_stop_market_combination(
    context: &str,
    field_path: &str,
    order: &NtOrderTemplateConfig,
) -> Vec<String> {
    let mut errors = Vec::new();
    if order.is_post_only {
        errors.push(format!(
            "{context}: {field_path}.is_post_only must be false for order_type=trailing_stop_market"
        ));
    }
    if order
        .trigger_price
        .is_some_and(|value| value <= Decimal::ZERO)
    {
        errors.push(format!(
            "{context}: {field_path}.trigger_price must be positive for order_type=trailing_stop_market"
        ));
    }
    if order
        .activation_price
        .is_some_and(|value| value <= Decimal::ZERO)
    {
        errors.push(format!(
            "{context}: {field_path}.activation_price must be positive for order_type=trailing_stop_market"
        ));
    }
    if order.trigger_price.is_none()
        && order
            .activation_price
            .is_none_or(|value| value <= Decimal::ZERO)
    {
        errors.push(format!(
            "{context}: {field_path}.trigger_price or activation_price must be positive for order_type=trailing_stop_market"
        ));
    }
    if order
        .trailing_offset
        .is_none_or(|value| value <= Decimal::ZERO)
    {
        errors.push(format!(
            "{context}: {field_path}.trailing_offset must be positive for order_type=trailing_stop_market"
        ));
    }
    if order.time_in_force == TimeInForce::Gtd
        && order.expire_time_unix_nanos.is_none_or(|value| value == 0)
    {
        errors.push(format!(
            "{context}: {field_path}.expire_time_unix_nanos is required for GTD trailing_stop_market orders"
        ));
    }
    errors
}

pub fn validate_nt_order_template(
    prefix: &str,
    template: &NtOrderTemplate,
    inputs: &NtOrderBuildInputs,
) -> Result<()> {
    if matches!(
        template.order_type,
        OrderType::MarketToLimit | OrderType::TrailingStopLimit
    ) {
        return Err(unsupported_nt_order_type_error(prefix, template.order_type));
    }

    match (
        template.order_type,
        template.time_in_force,
        template.trigger_price,
    ) {
        (OrderType::Limit, TimeInForce::Gtd, _)
            if template.expire_time.is_none_or(|value| value.as_u64() == 0) =>
        {
            anyhow::bail!("{prefix}_expire_time is required for GTD limit orders")
        }
        (OrderType::StopMarket | OrderType::MarketIfTouched, TimeInForce::Gtd, _)
            if template.expire_time.is_none_or(|value| value.as_u64() == 0) =>
        {
            anyhow::bail!("{prefix}_expire_time is required for GTD triggered-market orders")
        }
        (OrderType::StopLimit, TimeInForce::Gtd, _)
            if template.expire_time.is_none_or(|value| value.as_u64() == 0) =>
        {
            anyhow::bail!("{prefix}_expire_time is required for GTD StopLimit orders")
        }
        (OrderType::LimitIfTouched, TimeInForce::Gtd, _)
            if template.expire_time.is_none_or(|value| value.as_u64() == 0) =>
        {
            anyhow::bail!("{prefix}_expire_time is required for GTD LimitIfTouched orders")
        }
        (OrderType::TrailingStopMarket, TimeInForce::Gtd, _)
            if template.expire_time.is_none_or(|value| value.as_u64() == 0) =>
        {
            anyhow::bail!("{prefix}_expire_time is required for GTD TrailingStopMarket orders")
        }
        (OrderType::Market, TimeInForce::Gtd, _) => {
            anyhow::bail!("GTD not supported for Market orders")
        }
        (OrderType::Market, _, _) if template.expire_time.is_some() => {
            anyhow::bail!("{prefix}_expire_time is not supported for Market orders")
        }
        (
            OrderType::StopMarket
            | OrderType::StopLimit
            | OrderType::MarketIfTouched
            | OrderType::LimitIfTouched,
            _,
            None,
        ) => anyhow::bail!("{prefix}_trigger_price is required for triggered orders"),
        (OrderType::Limit | OrderType::Market, _, Some(_)) => {
            anyhow::bail!("{prefix}_trigger_price is only supported for triggered orders")
        }
        _ => {}
    }

    match template.order_type {
        OrderType::Market if template.is_post_only => {
            anyhow::bail!("{prefix}_is_post_only must be false for market orders")
        }
        OrderType::StopMarket if template.is_post_only => {
            anyhow::bail!("{prefix}_is_post_only must be false for StopMarket orders")
        }
        OrderType::MarketIfTouched if template.is_post_only => {
            anyhow::bail!("{prefix}_is_post_only must be false for MarketIfTouched orders")
        }
        _ => {}
    }

    if !matches!(template.order_type, OrderType::TrailingStopMarket) {
        anyhow::ensure!(
            template.activation_price.is_none(),
            "{prefix}_activation_price is only supported for TrailingStopMarket orders"
        );
        anyhow::ensure!(
            template.trailing_offset.is_none(),
            "{prefix}_trailing_offset is only supported for TrailingStopMarket orders"
        );
        anyhow::ensure!(
            template.trailing_offset_type.is_none(),
            "{prefix}_trailing_offset_type is only supported for TrailingStopMarket orders"
        );
    }

    if matches!(template.order_type, OrderType::Limit | OrderType::Market) {
        anyhow::ensure!(
            template.trigger_type.is_none(),
            "{prefix}_trigger_type is only supported for triggered orders"
        );
        anyhow::ensure!(
            template.trigger_instrument_id.is_none(),
            "{prefix}_trigger_instrument_id is only supported for triggered orders"
        );
    }

    if matches!(template.order_type, OrderType::TrailingStopMarket) {
        anyhow::ensure!(
            !template.is_post_only,
            "{prefix}_is_post_only must be false for TrailingStopMarket orders"
        );
        if let Some(trigger_price) = template.trigger_price {
            anyhow::ensure!(
                trigger_price.is_positive(),
                "{prefix}_trigger_price must be positive"
            );
        }
        if let Some(activation_price) = template.activation_price {
            anyhow::ensure!(
                activation_price.is_positive(),
                "{prefix}_activation_price must be positive"
            );
        }
        anyhow::ensure!(
            template
                .trigger_price
                .is_some_and(|price| price.is_positive())
                || template
                    .activation_price
                    .is_some_and(|price| price.is_positive()),
            "{prefix}_trigger_price or {prefix}_activation_price is required for TrailingStopMarket orders"
        );
        let trailing_offset = template.trailing_offset.ok_or_else(|| {
            anyhow::anyhow!("{prefix}_trailing_offset is required for TrailingStopMarket orders")
        })?;
        anyhow::ensure!(
            trailing_offset > Decimal::ZERO,
            "{prefix}_trailing_offset must be positive"
        );
    }

    let Some(trigger_price) = template.trigger_price else {
        return Ok(());
    };
    if matches!(
        template.order_type,
        OrderType::StopMarket
            | OrderType::StopLimit
            | OrderType::MarketIfTouched
            | OrderType::LimitIfTouched
    ) {
        anyhow::ensure!(
            trigger_price.is_positive(),
            "{prefix}_trigger_price must be positive"
        );
    }
    if template.order_type == OrderType::LimitIfTouched {
        let price = inputs.price.ok_or_else(|| {
            anyhow::anyhow!("{prefix}_price is required for LimitIfTouched orders")
        })?;
        match inputs.order_side {
            OrderSide::Buy if trigger_price > price => anyhow::bail!(
                "{prefix}_trigger_price must be <= order price for BUY LimitIfTouched orders"
            ),
            OrderSide::Sell if trigger_price < price => anyhow::bail!(
                "{prefix}_trigger_price must be >= order price for SELL LimitIfTouched orders"
            ),
            _ => {}
        }
    }
    Ok(())
}

fn unsupported_nt_order_type_error(prefix: &str, order_type: OrderType) -> anyhow::Error {
    anyhow::anyhow!(
        "{prefix}_order_type `{}` is not exposed by the pinned NT single-order OrderFactory",
        order_type
    )
}

fn required_limit_price(
    prefix: &str,
    order_type: OrderType,
    price: Option<Price>,
) -> Result<Price> {
    price.ok_or_else(|| anyhow::anyhow!("{prefix}_price is required for {order_type:?} orders"))
}

pub fn build_nt_order(
    order_factory: &mut OrderFactory,
    prefix: &str,
    template: &NtOrderTemplate,
    inputs: NtOrderBuildInputs,
) -> Result<OrderAny> {
    validate_nt_order_template(prefix, template, &inputs)?;

    match template.order_type {
        OrderType::Limit => Ok(order_factory.limit(
            inputs.instrument_id,
            inputs.order_side,
            inputs.quantity,
            required_limit_price(prefix, template.order_type, inputs.price)?,
            Some(template.time_in_force),
            template.expire_time,
            Some(template.is_post_only),
            Some(template.is_reduce_only),
            Some(template.is_quote_quantity),
            None,
            None,
            None,
            None,
            None,
            None,
            Some(inputs.client_order_id),
        )),
        OrderType::Market => Ok(order_factory.market(
            inputs.instrument_id,
            inputs.order_side,
            inputs.quantity,
            Some(template.time_in_force),
            Some(template.is_reduce_only),
            Some(template.is_quote_quantity),
            None,
            None,
            None,
            Some(inputs.client_order_id),
        )),
        OrderType::StopMarket => Ok(order_factory.stop_market(
            inputs.instrument_id,
            inputs.order_side,
            inputs.quantity,
            template
                .trigger_price
                .expect("validated StopMarket trigger price"),
            template.trigger_type,
            Some(template.time_in_force),
            template.expire_time,
            Some(template.is_reduce_only),
            Some(template.is_quote_quantity),
            None,
            None,
            template.trigger_instrument_id,
            None,
            None,
            None,
            Some(inputs.client_order_id),
        )),
        OrderType::MarketIfTouched => Ok(order_factory.market_if_touched(
            inputs.instrument_id,
            inputs.order_side,
            inputs.quantity,
            template
                .trigger_price
                .expect("validated MarketIfTouched trigger price"),
            template.trigger_type,
            Some(template.time_in_force),
            template.expire_time,
            Some(template.is_reduce_only),
            Some(template.is_quote_quantity),
            None,
            template.trigger_instrument_id,
            None,
            None,
            None,
            Some(inputs.client_order_id),
        )),
        OrderType::StopLimit => Ok(order_factory.stop_limit(
            inputs.instrument_id,
            inputs.order_side,
            inputs.quantity,
            required_limit_price(prefix, template.order_type, inputs.price)?,
            template
                .trigger_price
                .expect("validated triggered order trigger price"),
            template.trigger_type,
            Some(template.time_in_force),
            template.expire_time,
            Some(template.is_post_only),
            Some(template.is_reduce_only),
            Some(template.is_quote_quantity),
            None,
            None,
            template.trigger_instrument_id,
            None,
            None,
            None,
            Some(inputs.client_order_id),
        )),
        OrderType::LimitIfTouched => Ok(order_factory.limit_if_touched(
            inputs.instrument_id,
            inputs.order_side,
            inputs.quantity,
            required_limit_price(prefix, template.order_type, inputs.price)?,
            template
                .trigger_price
                .expect("validated LimitIfTouched trigger price"),
            template.trigger_type,
            Some(template.time_in_force),
            template.expire_time,
            Some(template.is_post_only),
            Some(template.is_reduce_only),
            Some(template.is_quote_quantity),
            None,
            None,
            template.trigger_instrument_id,
            None,
            None,
            None,
            Some(inputs.client_order_id),
        )),
        OrderType::TrailingStopMarket => {
            let activation_price = template.activation_price;
            let trigger_price = template.trigger_price.or(activation_price);
            Ok(order_factory.trailing_stop_market(
                inputs.instrument_id,
                inputs.order_side,
                inputs.quantity,
                template
                    .trailing_offset
                    .expect("validated TrailingStopMarket trailing offset"),
                template.trailing_offset_type,
                activation_price,
                trigger_price,
                template.trigger_type,
                Some(template.time_in_force),
                template.expire_time,
                Some(template.is_reduce_only),
                Some(template.is_quote_quantity),
                None,
                None,
                template.trigger_instrument_id,
                None,
                None,
                None,
                Some(inputs.client_order_id),
            ))
        }
        _ => Err(unsupported_nt_order_type_error(prefix, template.order_type)),
    }
}
