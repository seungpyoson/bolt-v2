use anyhow::{Context, Result};
use nautilus_model::{
    enums::{
        OmsType as NtOmsType, OrderSide, OrderType, PositionSide, TimeInForce, TrailingOffsetType,
        TriggerType,
    },
    identifiers::{ClientOrderId, InstrumentId},
    orders::OrderAny,
    types::{Price, Quantity},
};
use nautilus_trading::StrategyNative;

use crate::bolt_v3_order_intent::{NtOrderBuildInputs, NtOrderTemplate, build_nt_order};

use super::{
    BinaryOracleEdgeTaker, CONFIG_FIELD_EXIT_ORDER_POSITION_SIDE, CONFIG_FIELD_EXIT_ORDER_SIDE,
    CONFIG_FIELD_FORCED_EXIT_ORDER_POSITION_SIDE, CONFIG_FIELD_FORCED_EXIT_ORDER_SIDE,
    ORDER_CONFIGURATION_PREFIX_ENTRY, ORDER_CONFIGURATION_PREFIX_EXIT,
    activation_price_from_config, config::BinaryOracleEdgeTakerOrderConfig,
    expire_time_from_config, trailing_offset_from_config, trigger_price_from_config,
};

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct ConfiguredNtOrderTemplate {
    pub(super) order_type: OrderType,
    pub(super) time_in_force: TimeInForce,
    pub(super) expire_time_unix_nanos: Option<u64>,
    pub(super) trigger_price: Option<f64>,
    pub(super) activation_price: Option<f64>,
    pub(super) trigger_type: Option<TriggerType>,
    pub(super) trigger_instrument_id: Option<InstrumentId>,
    pub(super) trailing_offset: Option<f64>,
    pub(super) trailing_offset_type: Option<TrailingOffsetType>,
    pub(super) is_post_only: bool,
    pub(super) is_reduce_only: bool,
    pub(super) is_quote_quantity: bool,
}

impl ConfiguredNtOrderTemplate {
    fn nt_order_template(
        &self,
        prefix: &'static str,
        price_precision: u8,
    ) -> Result<NtOrderTemplate> {
        Ok(NtOrderTemplate {
            order_type: self.order_type,
            time_in_force: self.time_in_force,
            expire_time: expire_time_from_config(self.expire_time_unix_nanos),
            trigger_price: trigger_price_from_config(prefix, self.trigger_price, price_precision)?,
            activation_price: activation_price_from_config(
                prefix,
                self.activation_price,
                price_precision,
            )?,
            trigger_type: self.trigger_type,
            trigger_instrument_id: self.trigger_instrument_id,
            trailing_offset: trailing_offset_from_config(prefix, self.trailing_offset)?,
            trailing_offset_type: self.trailing_offset_type,
            is_post_only: self.is_post_only,
            is_reduce_only: self.is_reduce_only,
            is_quote_quantity: self.is_quote_quantity,
        })
    }
}

impl From<&BinaryOracleEdgeTakerOrderConfig> for ConfiguredNtOrderTemplate {
    fn from(order: &BinaryOracleEdgeTakerOrderConfig) -> Self {
        Self {
            order_type: order.order_type,
            time_in_force: order.time_in_force,
            expire_time_unix_nanos: order.expire_time_unix_nanos,
            trigger_price: order.trigger_price,
            activation_price: order.activation_price,
            trigger_type: order.trigger_type,
            trigger_instrument_id: order.trigger_instrument_id,
            trailing_offset: order.trailing_offset,
            trailing_offset_type: order.trailing_offset_type,
            is_post_only: order.is_post_only,
            is_reduce_only: order.is_reduce_only,
            is_quote_quantity: order.is_quote_quantity,
        }
    }
}

impl BinaryOracleEdgeTakerOrderConfig {
    fn nt_order_template(
        &self,
        prefix: &'static str,
        price_precision: u8,
    ) -> Result<NtOrderTemplate> {
        ConfiguredNtOrderTemplate::from(self).nt_order_template(prefix, price_precision)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct ExitOrderExecutionConfig {
    pub(super) side: OrderSide,
    pub(super) position_side: PositionSide,
    pub(super) order_template: ConfiguredNtOrderTemplate,
}

impl ExitOrderExecutionConfig {
    fn nt_order_template(
        &self,
        prefix: &'static str,
        price_precision: u8,
    ) -> Result<NtOrderTemplate> {
        self.order_template
            .nt_order_template(prefix, price_precision)
    }
}

impl BinaryOracleEdgeTaker {
    pub(super) fn build_configured_entry_order(
        &mut self,
        instrument_id: InstrumentId,
        order_side: OrderSide,
        quantity: Quantity,
        price: Price,
        client_order_id: ClientOrderId,
    ) -> Result<OrderAny> {
        anyhow::ensure!(
            !self.config.entry_order.is_reduce_only,
            "entry_is_reduce_only must be false because binary_oracle_edge_taker entry orders open the managed position"
        );
        let template = self
            .config
            .entry_order
            .nt_order_template(ORDER_CONFIGURATION_PREFIX_ENTRY, price.precision)?;
        let mut order_factory = self.core.order_factory();
        build_nt_order(
            &mut order_factory,
            ORDER_CONFIGURATION_PREFIX_ENTRY,
            &template,
            NtOrderBuildInputs {
                instrument_id,
                order_side,
                quantity,
                price: Some(price),
                client_order_id,
            },
        )
    }

    fn exit_order_execution_config_from_order(
        &self,
        order: &BinaryOracleEdgeTakerOrderConfig,
        side_field: &'static str,
        position_side_field: &'static str,
    ) -> Result<ExitOrderExecutionConfig> {
        Ok(ExitOrderExecutionConfig {
            side: parse_configured_order_side(side_field, &order.side)?,
            position_side: parse_configured_position_side(
                position_side_field,
                &order.position_side,
            )?,
            order_template: ConfiguredNtOrderTemplate::from(order),
        })
    }

    pub(super) fn normal_exit_order_execution_config(&self) -> Result<ExitOrderExecutionConfig> {
        self.exit_order_execution_config_from_order(
            &self.config.exit_order,
            CONFIG_FIELD_EXIT_ORDER_SIDE,
            CONFIG_FIELD_EXIT_ORDER_POSITION_SIDE,
        )
    }

    fn forced_exit_order_execution_config(&self) -> Result<ExitOrderExecutionConfig> {
        self.exit_order_execution_config_from_order(
            &self.config.forced_exit_order,
            CONFIG_FIELD_FORCED_EXIT_ORDER_SIDE,
            CONFIG_FIELD_FORCED_EXIT_ORDER_POSITION_SIDE,
        )
    }

    pub(super) fn exit_order_execution_config(
        &self,
        forced_flat: bool,
    ) -> Result<ExitOrderExecutionConfig> {
        if forced_flat {
            self.forced_exit_order_execution_config()
        } else {
            self.normal_exit_order_execution_config()
        }
    }

    #[cfg(test)]
    pub(super) fn build_configured_exit_order(
        &mut self,
        instrument_id: InstrumentId,
        order_side: OrderSide,
        quantity: Quantity,
        price: Price,
        client_order_id: ClientOrderId,
    ) -> Result<OrderAny> {
        self.build_exit_order_with_execution_config(
            self.normal_exit_order_execution_config()?,
            instrument_id,
            order_side,
            quantity,
            price,
            client_order_id,
        )
    }

    pub(super) fn build_exit_order_with_execution_config(
        &mut self,
        order_config: ExitOrderExecutionConfig,
        instrument_id: InstrumentId,
        order_side: OrderSide,
        quantity: Quantity,
        price: Price,
        client_order_id: ClientOrderId,
    ) -> Result<OrderAny> {
        anyhow::ensure!(
            !order_config.order_template.is_quote_quantity,
            "exit_is_quote_quantity must be false because exits are sized from base position quantity"
        );
        let template =
            order_config.nt_order_template(ORDER_CONFIGURATION_PREFIX_EXIT, price.precision)?;
        let mut order_factory = self.core.order_factory();
        build_nt_order(
            &mut order_factory,
            ORDER_CONFIGURATION_PREFIX_EXIT,
            &template,
            NtOrderBuildInputs {
                instrument_id,
                order_side,
                quantity,
                price: Some(price),
                client_order_id,
            },
        )
    }
}

const ORDER_SIDE_BUY_VALUE: &str = stringify!(buy);
const ORDER_SIDE_SELL_VALUE: &str = stringify!(sell);
const POSITION_SIDE_LONG_VALUE: &str = stringify!(long);
const POSITION_SIDE_SHORT_VALUE: &str = stringify!(short);

pub(super) fn parse_configured_order_side(field: &str, value: &str) -> Result<OrderSide> {
    match value {
        ORDER_SIDE_BUY_VALUE => Ok(OrderSide::Buy),
        ORDER_SIDE_SELL_VALUE => Ok(OrderSide::Sell),
        _ => anyhow::bail!("{field} must be `buy` or `sell`, got `{value}`"),
    }
}

pub(super) fn parse_configured_position_side(field: &str, value: &str) -> Result<PositionSide> {
    match value {
        POSITION_SIDE_LONG_VALUE => Ok(PositionSide::Long),
        POSITION_SIDE_SHORT_VALUE => Ok(PositionSide::Short),
        _ => anyhow::bail!("{field} must be `long` or `short`, got `{value}`"),
    }
}

pub(super) fn parse_configured_oms_type(field: &str, value: &str) -> Result<NtOmsType> {
    value
        .parse::<NtOmsType>()
        .with_context(|| format!("{field} must be a NautilusTrader OmsType, got `{value}`"))
}

#[cfg(test)]
#[derive(Debug, Clone, PartialEq)]
pub(super) struct EntryOrderPlanInputs {
    pub(super) client_order_id: ClientOrderId,
    pub(super) instrument_id: InstrumentId,
    pub(super) order_side: OrderSide,
    pub(super) quantity: Quantity,
    pub(super) price_precision: u8,
    pub(super) time_in_force: TimeInForce,
    pub(super) best_bid: f64,
    pub(super) best_ask: f64,
}

#[cfg(test)]
#[derive(Debug, Clone, PartialEq)]
pub(super) struct EntryOrderPlan {
    pub(super) client_order_id: ClientOrderId,
    pub(super) instrument_id: InstrumentId,
    pub(super) order_side: OrderSide,
    pub(super) quantity: Quantity,
    pub(super) price: Price,
    pub(super) time_in_force: TimeInForce,
}

#[cfg(test)]
pub(super) fn build_entry_order_plan(inputs: &EntryOrderPlanInputs) -> Result<EntryOrderPlan> {
    let raw_price = match inputs.order_side {
        OrderSide::Buy => inputs.best_ask,
        OrderSide::Sell => inputs.best_bid,
        _ => anyhow::bail!(
            "entry order side must be `buy` or `sell`, got `{:?}`",
            inputs.order_side
        ),
    };
    anyhow::ensure!(
        raw_price.is_finite() && raw_price > 0.0,
        "entry price must be positive"
    );

    Ok(EntryOrderPlan {
        client_order_id: inputs.client_order_id,
        instrument_id: inputs.instrument_id,
        order_side: inputs.order_side,
        quantity: inputs.quantity,
        price: Price::new(raw_price, inputs.price_precision),
        time_in_force: inputs.time_in_force,
    })
}
