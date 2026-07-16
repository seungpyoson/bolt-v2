//! Strategy-archetype contract for the first outcome-group consumer.
//!
//! This module owns the complete-set validation and raw NT strategy config
//! mapping. Builder registration stays in the strategy layer.

use std::collections::BTreeSet;

use nautilus_live::node::LiveNode;
use nautilus_model::identifiers::StrategyId;
use rust_decimal::Decimal;
use toml::{Value, map::Map};

use crate::{
    bolt_v3_archetypes::{ArchetypeGateRequirement, ArchetypeValidationBinding},
    bolt_v3_complete_set_contract::{
        COMPLETE_SET_ARBITRAGE_KEY, CompleteSetArbitrageParametersBlock, CompleteSetSubmitMode,
        submit_mode_contract,
    },
    bolt_v3_config::{BoltV3RootConfig, BoltV3StrategyConfig, LoadedBoltV3Config, LoadedStrategy},
    bolt_v3_strategy_registration::{
        BoltV3StrategyRegistrationError, StrategyRegistrationContext, StrategyRuntimeBinding,
        StrategyRuntimeCapabilities, assemble_strategy_build_context, venue_for_client,
    },
    strategies::{
        complete_set_arbitrage::CompleteSetArbitrageBuilder, production_strategy_registry,
        registry::StrategyBuilder,
    },
};

pub const KEY: &str = COMPLETE_SET_ARBITRAGE_KEY;

/// Complete-set runtime binding assembled by the strategy-layer archetype module.
pub const RUNTIME_BINDING: StrategyRuntimeBinding = StrategyRuntimeBinding {
    key: KEY,
    strategy_kind: CompleteSetArbitrageBuilder::kind,
    capabilities: StrategyRuntimeCapabilities {
        realized_volatility: true,
        settlement: false,
    },
    register: register_runtime_strategy,
};

pub fn validation_binding() -> ArchetypeValidationBinding {
    ArchetypeValidationBinding {
        key: KEY,
        validate_strategy,
    }
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

pub fn validate_strategy(
    context: &str,
    _root: &BoltV3RootConfig,
    strategy: &BoltV3StrategyConfig,
    default_max_notional_decimal: Option<&Decimal>,
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
    let mut nt_strategy_id = strategy.strategy_archetype.as_str().to_string();
    nt_strategy_id.push('-');
    nt_strategy_id.push_str(&strategy.order_id_tag);
    if let Err(error) = StrategyId::new_checked(&nt_strategy_id) {
        errors.push(format!(
            "{context}: strategy_id `{nt_strategy_id}` derived from order_id_tag is not a valid NT StrategyId ({error})"
        ));
    }

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
    if strategy.market_exit_reduce_only.is_none() {
        errors.push(format!("{context}: market_exit_reduce_only is required"));
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
        Ok(value) if value > Decimal::ZERO => {
            if let Some(default_max_notional) = default_max_notional_decimal
                && value > *default_max_notional
            {
                errors.push(format!(
                    "{context}: parameters.runtime.max_basket_notional must not exceed risk.default_max_notional_per_order"
                ));
            }
        }
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

#[derive(Debug)]
pub enum CompleteSetArbitrageRuntimeConfigError {
    WrongArchetype {
        expected: &'static str,
        actual: String,
    },
    Parameters {
        strategy_instance_id: String,
        message: String,
    },
    Client {
        strategy_instance_id: String,
        message: String,
    },
    Numeric {
        strategy_instance_id: String,
        field: &'static str,
        value: String,
    },
    StrategyId {
        strategy_instance_id: String,
        value: String,
        message: String,
    },
}

impl std::fmt::Display for CompleteSetArbitrageRuntimeConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::WrongArchetype { expected, actual } => {
                write!(
                    f,
                    "expected strategy archetype `{expected}`, got `{actual}`"
                )
            }
            Self::Parameters {
                strategy_instance_id,
                message,
            } => write!(
                f,
                "strategies.{strategy_instance_id} parameters are invalid: {message}"
            ),
            Self::Client {
                strategy_instance_id,
                message,
            } => write!(
                f,
                "strategies.{strategy_instance_id} client binding is invalid: {message}"
            ),
            Self::Numeric {
                strategy_instance_id,
                field,
                value,
            } => write!(
                f,
                "strategies.{strategy_instance_id} {field} cannot be represented for complete-set config: `{value}`"
            ),
            Self::StrategyId {
                strategy_instance_id,
                value,
                message,
            } => write!(
                f,
                "strategies.{strategy_instance_id} maps to invalid NT StrategyId `{value}`: {message}"
            ),
        }
    }
}

impl std::error::Error for CompleteSetArbitrageRuntimeConfigError {}

pub fn raw_complete_set_config(
    strategy: &LoadedStrategy,
    loaded: &LoadedBoltV3Config,
) -> Result<Value, CompleteSetArbitrageRuntimeConfigError> {
    if strategy.config.strategy_archetype.as_str() != KEY {
        return Err(CompleteSetArbitrageRuntimeConfigError::WrongArchetype {
            expected: KEY,
            actual: strategy.config.strategy_archetype.as_str().to_string(),
        });
    }

    venue_for_client(&loaded.root, strategy.config.execution_client_id.as_str()).ok_or_else(
        || CompleteSetArbitrageRuntimeConfigError::Client {
            strategy_instance_id: strategy.config.strategy_instance_id.clone(),
            message: format!(
                "execution_client_id `{}` is not present in loaded clients",
                strategy.config.execution_client_id
            ),
        },
    )?;

    let parameters = parse_parameters(&strategy.config.parameters).map_err(|message| {
        CompleteSetArbitrageRuntimeConfigError::Parameters {
            strategy_instance_id: strategy.config.strategy_instance_id.clone(),
            message,
        }
    })?;
    let runtime = parameters.runtime;
    let strategy_instance_id = strategy.config.strategy_instance_id.as_str();

    let mut table = Map::new();
    insert_string(&mut table, "strategy_id", nt_strategy_id(strategy)?);
    insert_string(
        &mut table,
        "order_id_tag",
        strategy.config.order_id_tag.clone(),
    );
    insert_string(
        &mut table,
        "oms_type",
        enum_variant_lowercase(strategy.config.oms_type),
    );
    insert_bool(
        &mut table,
        "use_uuid_client_order_ids",
        strategy.config.use_uuid_client_order_ids,
    );
    insert_bool(
        &mut table,
        "use_hyphens_in_client_order_ids",
        strategy.config.use_hyphens_in_client_order_ids,
    );
    insert_string_array(
        &mut table,
        "external_order_claims",
        &strategy.config.external_order_claims,
    );
    insert_bool(
        &mut table,
        "manage_contingent_orders",
        strategy.config.manage_contingent_orders,
    );
    insert_bool(
        &mut table,
        "manage_gtd_expiry",
        strategy.config.manage_gtd_expiry,
    );
    insert_bool(&mut table, "manage_stop", strategy.config.manage_stop);
    insert_u64(
        &mut table,
        strategy_instance_id,
        "market_exit_interval_ms",
        strategy.config.market_exit_interval_ms,
    )?;
    insert_u64(
        &mut table,
        strategy_instance_id,
        "market_exit_max_attempts",
        strategy.config.market_exit_max_attempts,
    )?;
    insert_bool(
        &mut table,
        "market_exit_reduce_only",
        strategy.config.market_exit_reduce_only.ok_or_else(|| {
            CompleteSetArbitrageRuntimeConfigError::Parameters {
                strategy_instance_id: strategy.config.strategy_instance_id.clone(),
                message: "market_exit_reduce_only is required".to_string(),
            }
        })?,
    );
    insert_bool(&mut table, "log_events", strategy.config.log_events);
    insert_bool(&mut table, "log_commands", strategy.config.log_commands);
    insert_bool(
        &mut table,
        "log_rejected_due_post_only_as_warning",
        strategy.config.log_rejected_due_post_only_as_warning,
    );
    insert_string(
        &mut table,
        "client_id",
        strategy.config.execution_client_id.to_string(),
    );
    insert_i64(&mut table, "min_edge_bps", runtime.min_edge_bps);
    insert_string(
        &mut table,
        "max_basket_notional",
        runtime.max_basket_notional,
    );
    insert_u32(&mut table, "max_open_baskets", runtime.max_open_baskets);
    insert_string(&mut table, "submit_mode", runtime.submit_mode);
    insert_u64(
        &mut table,
        strategy_instance_id,
        "vwap_depth_limit_bps",
        runtime.vwap_depth_limit_bps,
    )?;
    insert_u64(
        &mut table,
        strategy_instance_id,
        "slippage_buffer_bps",
        runtime.slippage_buffer_bps,
    )?;
    insert_u32(
        &mut table,
        "max_repair_attempts",
        runtime.max_repair_attempts,
    );
    insert_u32(
        &mut table,
        "max_unwind_attempts",
        runtime.max_unwind_attempts,
    );

    Ok(Value::Table(table))
}

pub fn register_runtime_strategy(
    node: &mut LiveNode,
    context: StrategyRegistrationContext<'_>,
) -> Result<StrategyId, BoltV3StrategyRegistrationError> {
    let raw = raw_complete_set_config(context.strategy, context.loaded)
        .map_err(|error| binding_message(&context, error.to_string()))?;
    let build_context = assemble_strategy_build_context(&context)?;
    let registry = production_strategy_registry()
        .map_err(|error| binding_message(&context, error.to_string()))?;
    registry
        .register_strategy(
            context.strategy_kind,
            &raw,
            &build_context,
            node.kernel().trader(),
        )
        .map_err(|error| binding_message(&context, error.to_string()))
}

fn binding_message(
    context: &StrategyRegistrationContext<'_>,
    message: String,
) -> BoltV3StrategyRegistrationError {
    BoltV3StrategyRegistrationError::Binding {
        strategy_instance_id: context.strategy.config.strategy_instance_id.clone(),
        strategy_archetype: context
            .strategy
            .config
            .strategy_archetype
            .as_str()
            .to_string(),
        message,
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

fn nt_strategy_id(
    strategy: &LoadedStrategy,
) -> Result<String, CompleteSetArbitrageRuntimeConfigError> {
    let mut value = strategy.config.strategy_archetype.as_str().to_string();
    value.push('-');
    value.push_str(&strategy.config.order_id_tag);
    StrategyId::new_checked(&value).map_err(|error| {
        CompleteSetArbitrageRuntimeConfigError::StrategyId {
            strategy_instance_id: strategy.config.strategy_instance_id.clone(),
            value: value.clone(),
            message: error.to_string(),
        }
    })?;
    Ok(value)
}

fn enum_variant_lowercase<T: std::fmt::Display>(value: T) -> String {
    value.to_string().to_ascii_lowercase()
}

fn insert_string(table: &mut Map<String, Value>, key: &'static str, value: String) {
    table.insert(key.to_string(), Value::String(value));
}

fn insert_bool(table: &mut Map<String, Value>, key: &'static str, value: bool) {
    table.insert(key.to_string(), Value::Boolean(value));
}

fn insert_string_array(table: &mut Map<String, Value>, key: &'static str, values: &[String]) {
    table.insert(
        key.to_string(),
        Value::Array(values.iter().cloned().map(Value::String).collect()),
    );
}

fn insert_i64(table: &mut Map<String, Value>, key: &'static str, value: i64) {
    table.insert(key.to_string(), Value::Integer(value));
}

fn insert_u32(table: &mut Map<String, Value>, key: &'static str, value: u32) {
    table.insert(key.to_string(), Value::Integer(i64::from(value)));
}

fn insert_u64(
    table: &mut Map<String, Value>,
    strategy_instance_id: &str,
    key: &'static str,
    value: u64,
) -> Result<(), CompleteSetArbitrageRuntimeConfigError> {
    let converted =
        i64::try_from(value).map_err(|_| CompleteSetArbitrageRuntimeConfigError::Numeric {
            strategy_instance_id: strategy_instance_id.to_string(),
            field: key,
            value: value.to_string(),
        })?;
    table.insert(key.to_string(), Value::Integer(converted));
    Ok(())
}
