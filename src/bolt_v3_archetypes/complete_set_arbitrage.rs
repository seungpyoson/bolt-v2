//! Strategy-archetype contract for the first outcome-group consumer.
//!
//! This module is intentionally source-only in this task. It declares the
//! complete-set validation and NT order-template contract, but it is not added
//! to the production validation/runtime binding arrays until the later runtime
//! activation task.

use std::collections::BTreeSet;

use nautilus_model::enums::{OrderType, TimeInForce};
use rust_decimal::Decimal;

use crate::{
    bolt_v3_archetypes::ArchetypeGateRequirement,
    bolt_v3_config::BoltV3StrategyConfig,
    bolt_v3_order_intent::{NtOrderTemplateConfig, check_nt_order_template_config},
    bolt_v3_outcome_group_sources::{
        COMPLETE_SET_ARBITRAGE_KEY, CompleteSetArbitrageParametersBlock,
    },
};

pub const KEY: &str = COMPLETE_SET_ARBITRAGE_KEY;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompleteSetSubmitMode {
    Ioc,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompleteSetSubmitModeContract {
    pub submit_mode: CompleteSetSubmitMode,
    pub order_template: NtOrderTemplateConfig,
    pub nt_template_errors: Vec<String>,
}

pub fn requires_realized_volatility_surface() -> bool {
    false
}

pub fn gate_requirements() -> Vec<ArchetypeGateRequirement> {
    Vec::new()
}

pub fn required_reference_data_roles() -> BTreeSet<&'static str> {
    BTreeSet::new()
}

pub fn optional_signal_gate_keys(parameters: &toml::Value) -> Result<BTreeSet<String>, String> {
    parse_parameters(parameters).map(|_| BTreeSet::new())
}

pub fn supported_submit_modes() -> Vec<CompleteSetSubmitMode> {
    vec![CompleteSetSubmitMode::Ioc]
}

pub fn submit_mode_contract(mode: CompleteSetSubmitMode) -> CompleteSetSubmitModeContract {
    let order_template = match mode {
        CompleteSetSubmitMode::Ioc => NtOrderTemplateConfig {
            order_type: OrderType::Market,
            time_in_force: TimeInForce::Ioc,
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
        },
    };
    let nt_template_errors = check_nt_order_template_config(
        KEY,
        "parameters.runtime.submit_mode.ioc.order_template",
        &order_template,
    );
    CompleteSetSubmitModeContract {
        submit_mode: mode,
        order_template,
        nt_template_errors,
    }
}

pub fn validate_strategy(
    context: &str,
    strategy: &BoltV3StrategyConfig,
    _default_max_notional_decimal: Option<&Decimal>,
) -> Vec<String> {
    let mut errors = Vec::new();
    if strategy.strategy_archetype.as_str() != KEY {
        errors.push(format!(
            "{context}: expected strategy_archetype `{KEY}`, got `{}`",
            strategy.strategy_archetype.as_str()
        ));
        return errors;
    }

    errors.extend(
        crate::bolt_v3_market_families::outcome_group::validate_target_block(
            context,
            &strategy.target,
        ),
    );

    for field in [
        "min_edge_bps",
        "max_basket_notional",
        "max_open_baskets",
        "submit_mode",
        "vwap_depth_limit_bps",
        "slippage_buffer_bps",
        "max_repair_attempts",
        "max_unwind_attempts",
    ] {
        if strategy
            .parameters
            .get("runtime")
            .and_then(toml::Value::as_table)
            .is_none_or(|runtime| !runtime.contains_key(field))
        {
            errors.push(format!("{context}: parameters.runtime.{field} is required"));
        }
    }

    let Ok(parameters) = parse_parameters(&strategy.parameters) else {
        if let Some(submit_mode) = strategy
            .parameters
            .get("runtime")
            .and_then(toml::Value::as_table)
            .and_then(|runtime| runtime.get("submit_mode"))
            .and_then(toml::Value::as_str)
            && CompleteSetSubmitMode::from_config(submit_mode).is_none()
        {
            errors.push(format!(
                "{context}: parameters.runtime.submit_mode `{submit_mode}` is not supported"
            ));
        }
        return errors;
    };

    let runtime = parameters.runtime;
    if runtime.min_edge_bps <= 0 {
        errors.push(format!(
            "{context}: parameters.runtime.min_edge_bps must be positive"
        ));
    }
    match runtime.max_basket_notional.parse::<Decimal>() {
        Ok(value) if value > Decimal::ZERO => {}
        Ok(_) | Err(_) => errors.push(format!(
            "{context}: parameters.runtime.max_basket_notional must be a positive decimal"
        )),
    }
    if runtime.max_open_baskets == 0 {
        errors.push(format!(
            "{context}: parameters.runtime.max_open_baskets must be positive"
        ));
    }
    if runtime.vwap_depth_limit_bps == 0 {
        errors.push(format!(
            "{context}: parameters.runtime.vwap_depth_limit_bps must be positive"
        ));
    }
    if runtime.slippage_buffer_bps == 0 {
        errors.push(format!(
            "{context}: parameters.runtime.slippage_buffer_bps must be positive"
        ));
    }
    if runtime.max_repair_attempts == 0 {
        errors.push(format!(
            "{context}: parameters.runtime.max_repair_attempts must be positive"
        ));
    }
    if runtime.max_unwind_attempts == 0 {
        errors.push(format!(
            "{context}: parameters.runtime.max_unwind_attempts must be positive"
        ));
    }

    match CompleteSetSubmitMode::from_config(runtime.submit_mode.as_str()) {
        Some(mode) => {
            errors.extend(
                submit_mode_contract(mode)
                    .nt_template_errors
                    .into_iter()
                    .map(|message| format!("{context}: {message}")),
            );
        }
        None => errors.push(format!(
            "{context}: parameters.runtime.submit_mode `{}` is not supported",
            runtime.submit_mode
        )),
    }

    errors
}

impl CompleteSetSubmitMode {
    fn from_config(value: &str) -> Option<Self> {
        match value {
            "ioc" => Some(Self::Ioc),
            _ => None,
        }
    }
}

fn parse_parameters(
    parameters: &toml::Value,
) -> Result<CompleteSetArbitrageParametersBlock, String> {
    parameters
        .clone()
        .try_into::<CompleteSetArbitrageParametersBlock>()
        .map_err(|error| error.to_string())
}
