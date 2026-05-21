//! Strategy-archetype binding for `binary_oracle_edge_taker`.
//!
//! This module owns:
//!
//! 1. The archetype's `[parameters]` block shape (`ParametersBlock`)
//!    and its `[parameters.entry_order]` / `[parameters.exit_order]`
//!    row shape (`OrderParams`). The `order_type` and `time_in_force`
//!    fields on `OrderParams` are typed with NT's canonical
//!    `nautilus_model::enums::{OrderType, TimeInForce}`; this archetype's
//!    validator allow-lists the specific combinations it supports rather
//!    than defining a narrower shadow enum. Core config in
//!    `crate::bolt_v3_config` keeps the strategy envelope and the
//!    field name `parameters`, but the row shape is archetype-specific
//!    and lives here so a future archetype can introduce its own
//!    parameter row without reaching back into core config.
//! 2. The archetype's bolt-v3 startup-validation policy:
//!    - the required reference-data role
//!      (`[reference_data.primary]`),
//!    - the supported `[parameters.entry_order]` and
//!      `[parameters.exit_order]` combinations: taker limit/FOK or
//!      market/IOC where applicable, maker limit/GTC with
//!      `is_post_only=true`, limit/GTD with an explicit
//!      `expire_time_unix_nanos`, and stop-market with an explicit
//!      `trigger_price`.
//!
//! Core startup validation in `crate::bolt_v3_validate` keeps target-
//! shape and per-role reference-data structural checks structural and
//! dispatches archetype-specific rules through
//! `crate::bolt_v3_archetypes::validate_strategy_archetype` based on
//! `strategy.strategy_archetype`. Archetype-specific error-message
//! policy (the headline "is not allowed for `binary_oracle_edge_taker`"
//! phrase, the per-field rule listing, and the
//! "requires `[reference_data.primary]`" phrase) lives here so that a
//! future archetype can introduce its own message contract without
//! reaching back into core validation.

use rust_decimal::{Decimal, prelude::ToPrimitive};
use serde::Deserialize;
use toml::{Value, map::Map};

use nautilus_model::{
    enums::{OrderSide, OrderType, PositionSide, TimeInForce},
    identifiers::StrategyId,
};

use crate::{
    bolt_v3_archetypes::ArchetypeValidationBinding,
    bolt_v3_config::{BoltV3StrategyConfig, LoadedStrategy},
    bolt_v3_providers::polymarket,
    bolt_v3_strategy_registration::{
        BoltV3StrategyRegistrationError, StrategyRegistrationContext, StrategyRuntimeBinding,
    },
    strategies::{
        binary_oracle_edge_taker::{BinaryOracleEdgeTakerBuilder, KEY as STRATEGY_KIND},
        production_strategy_registry,
        registry::{StrategyBuildContext, StrategyBuilder},
    },
};

pub const KEY: &str = STRATEGY_KIND;

pub fn validation_binding() -> ArchetypeValidationBinding {
    ArchetypeValidationBinding {
        key: KEY,
        validate_strategy,
    }
}

pub const RUNTIME_BINDING: StrategyRuntimeBinding = StrategyRuntimeBinding {
    key: KEY,
    strategy_kind: BinaryOracleEdgeTakerBuilder::kind,
    register: register_runtime_strategy,
};

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ParametersBlock {
    pub edge_threshold_basis_points: i64,
    pub order_notional_target: String,
    pub maximum_position_notional: String,
    pub runtime: RuntimeParametersBlock,
    pub entry_order: OrderParams,
    pub exit_order: OrderParams,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RuntimeParametersBlock {
    pub reference_publish_topic: String,
    pub warmup_tick_count: u64,
    pub reentry_cooldown_secs: u64,
    pub book_impact_cap_bps: u64,
    pub risk_lambda: f64,
    pub exit_hysteresis_bps: i64,
    pub vol_window_secs: u64,
    pub vol_gap_reset_secs: u64,
    pub vol_min_observations: u64,
    pub vol_bridge_valid_secs: u64,
    pub price_to_beat_source: String,
    pub pricing_kurtosis: f64,
    pub theta_decay_factor: f64,
    pub forced_flat_stale_chainlink_ms: u64,
    pub forced_flat_thin_book_min_liquidity: f64,
    pub lead_agreement_min_corr: f64,
    pub lead_jitter_max_ms: u64,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct OrderParams {
    pub side: OrderSide,
    pub position_side: PositionSide,
    pub order_type: OrderType,
    pub time_in_force: TimeInForce,
    pub expire_time_unix_nanos: Option<u64>,
    pub trigger_price: Option<Decimal>,
    pub is_post_only: bool,
    pub is_reduce_only: bool,
    pub is_quote_quantity: bool,
}

pub fn validate_strategy(
    context: &str,
    strategy: &BoltV3StrategyConfig,
    default_max_notional: Option<&Decimal>,
) -> Vec<String> {
    let mut errors = validate_required_reference_data(context, strategy);

    let parameters = match strategy.parameters.clone().try_into::<ParametersBlock>() {
        Ok(value) => value,
        Err(error) => {
            errors.push(format!(
                "{context}: parameters block is not a valid `binary_oracle_edge_taker` [parameters] block: {error}"
            ));
            return errors;
        }
    };

    errors.extend(validate_order_parameters(
        context,
        &parameters.entry_order,
        &parameters.exit_order,
    ));
    errors.extend(validate_parameter_bounds(
        context,
        &parameters,
        default_max_notional,
    ));
    errors
}

#[derive(Debug)]
pub enum BinaryOracleEdgeTakerRuntimeConfigError {
    WrongArchetype {
        expected: &'static str,
        actual: String,
    },
    Parameters {
        strategy_instance_id: String,
        message: String,
    },
    Target {
        strategy_instance_id: String,
        message: String,
    },
    ReferenceData {
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

impl std::fmt::Display for BinaryOracleEdgeTakerRuntimeConfigError {
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
            Self::Target {
                strategy_instance_id,
                message,
            } => write!(
                f,
                "strategies.{strategy_instance_id} target is invalid: {message}"
            ),
            Self::ReferenceData {
                strategy_instance_id,
                message,
            } => write!(
                f,
                "strategies.{strategy_instance_id} reference_data is invalid: {message}"
            ),
            Self::Numeric {
                strategy_instance_id,
                field,
                value,
            } => write!(
                f,
                "strategies.{strategy_instance_id} {field} cannot be represented for existing taker config: `{value}`"
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

impl std::error::Error for BinaryOracleEdgeTakerRuntimeConfigError {}

pub fn register_runtime_strategy(
    node: &mut nautilus_live::node::LiveNode,
    context: StrategyRegistrationContext<'_>,
) -> Result<StrategyId, BoltV3StrategyRegistrationError> {
    let raw = raw_taker_config(context.strategy, context.loaded)
        .map_err(|error| binding_error(&context, error))?;
    let client = context
        .loaded
        .root
        .clients
        .get(context.strategy.config.execution_client_id.as_str())
        .ok_or_else(|| {
            binding_message(
                &context,
                format!(
                    "strategy execution_client_id `{}` is not present in loaded clients",
                    context.strategy.config.execution_client_id
                ),
            )
        })?;
    let fee_provider = polymarket::build_fee_provider(
        context.strategy.config.execution_client_id.as_str(),
        client,
        context.resolved,
    )
    .map_err(|error| binding_message(&context, error.to_string()))?;
    let build_context = StrategyBuildContext::new(
        fee_provider,
        context.decision_evidence.clone(),
        context.submit_admission.clone(),
    );
    let registry = production_strategy_registry()
        .map_err(|error| binding_message(&context, error.to_string()))?;
    registry
        .register_strategy(
            BinaryOracleEdgeTakerBuilder::kind(),
            &raw,
            &build_context,
            node.kernel().trader(),
        )
        .map_err(|error| binding_message(&context, error.to_string()))
}

pub fn raw_taker_config(
    strategy: &LoadedStrategy,
    loaded: &crate::bolt_v3_config::LoadedBoltV3Config,
) -> Result<Value, BinaryOracleEdgeTakerRuntimeConfigError> {
    if strategy.config.strategy_archetype.as_str() != KEY {
        return Err(BinaryOracleEdgeTakerRuntimeConfigError::WrongArchetype {
            expected: KEY,
            actual: strategy.config.strategy_archetype.as_str().to_string(),
        });
    }

    let parameters = parameters_block(strategy)?;
    let target =
        crate::bolt_v3_market_families::target_runtime_fields_from_target(&strategy.config.target)
            .map_err(|error| BinaryOracleEdgeTakerRuntimeConfigError::Target {
                strategy_instance_id: strategy.config.strategy_instance_id.clone(),
                message: error.to_string(),
            })?;
    loaded
        .root
        .clients
        .get(strategy.config.execution_client_id.as_str())
        .ok_or_else(|| BinaryOracleEdgeTakerRuntimeConfigError::ReferenceData {
            strategy_instance_id: strategy.config.strategy_instance_id.clone(),
            message: format!(
                "execution_client_id `{}` is not present in loaded clients",
                strategy.config.execution_client_id
            ),
        })?;
    let reference_data = configured_reference_data(strategy)?;
    loaded
        .root
        .clients
        .get(reference_data.data_client_id.as_str())
        .ok_or_else(|| BinaryOracleEdgeTakerRuntimeConfigError::ReferenceData {
            strategy_instance_id: strategy.config.strategy_instance_id.clone(),
            message: format!(
                "reference_data data_client_id `{}` is not present in loaded clients",
                reference_data.data_client_id
            ),
        })?;

    let order_notional_target = decimal_string_to_f64(
        &strategy.config.strategy_instance_id,
        "parameters.order_notional_target",
        &parameters.order_notional_target,
    )?;
    let maximum_position_notional = decimal_string_to_f64(
        &strategy.config.strategy_instance_id,
        "parameters.maximum_position_notional",
        &parameters.maximum_position_notional,
    )?;
    let cadence_seconds = i64_to_u64(
        &strategy.config.strategy_instance_id,
        target.cadence_seconds_source_field,
        target.cadence_seconds,
    )?;

    let strategy_instance_id = strategy.config.strategy_instance_id.as_str();
    let mut table = Map::new();
    insert_string(&mut table, "strategy_id", nt_strategy_id(strategy)?);
    insert_string(
        &mut table,
        "order_id_tag",
        strategy.config.order_id_tag.clone(),
    );
    insert_string(&mut table, "oms_type", oms_type_value(strategy));
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
    insert_string(
        &mut table,
        "market_exit_time_in_force",
        strategy.config.market_exit_time_in_force.clone(),
    );
    insert_bool(
        &mut table,
        "market_exit_reduce_only",
        strategy.config.market_exit_reduce_only,
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
    insert_string(
        &mut table,
        "configured_target_id",
        target.configured_target_id,
    );
    insert_string(&mut table, "target_kind", target.target_kind);
    insert_string(
        &mut table,
        "rotating_market_family",
        target.rotating_market_family,
    );
    insert_string(&mut table, "underlying_asset", target.underlying_asset);
    insert_u64(
        &mut table,
        strategy_instance_id,
        "cadence_seconds",
        cadence_seconds,
    )?;
    insert_string(&mut table, "cadence_slug_token", target.cadence_slug_token);
    insert_string(
        &mut table,
        "market_selection_rule",
        target.market_selection_rule,
    );
    insert_u64(
        &mut table,
        strategy_instance_id,
        "retry_interval_seconds",
        target.retry_interval_seconds,
    )?;
    insert_u64(
        &mut table,
        strategy_instance_id,
        "blocked_after_seconds",
        target.blocked_after_seconds,
    )?;
    insert_string(
        &mut table,
        "reference_venue",
        reference_data.data_client_id.to_string(),
    );
    insert_string(
        &mut table,
        "reference_instrument_id",
        reference_data.instrument_id.to_string(),
    );
    insert_order_config(
        &mut table,
        strategy_instance_id,
        "entry_order",
        &parameters.entry_order,
    )?;
    insert_order_config(
        &mut table,
        strategy_instance_id,
        "exit_order",
        &parameters.exit_order,
    )?;
    insert_u64(
        &mut table,
        strategy_instance_id,
        "warmup_tick_count",
        parameters.runtime.warmup_tick_count,
    )?;
    insert_u64(
        &mut table,
        strategy_instance_id,
        "reentry_cooldown_secs",
        parameters.runtime.reentry_cooldown_secs,
    )?;
    insert_float(&mut table, "order_notional_target", order_notional_target);
    insert_float(
        &mut table,
        "maximum_position_notional",
        maximum_position_notional,
    );
    insert_u64(
        &mut table,
        strategy_instance_id,
        "book_impact_cap_bps",
        parameters.runtime.book_impact_cap_bps,
    )?;
    insert_float(&mut table, "risk_lambda", parameters.runtime.risk_lambda);
    insert_i64(
        &mut table,
        "edge_threshold_basis_points",
        parameters.edge_threshold_basis_points,
    );
    insert_i64(
        &mut table,
        "exit_hysteresis_bps",
        parameters.runtime.exit_hysteresis_bps,
    );
    insert_u64(
        &mut table,
        strategy_instance_id,
        "vol_window_secs",
        parameters.runtime.vol_window_secs,
    )?;
    insert_u64(
        &mut table,
        strategy_instance_id,
        "vol_gap_reset_secs",
        parameters.runtime.vol_gap_reset_secs,
    )?;
    insert_u64(
        &mut table,
        strategy_instance_id,
        "vol_min_observations",
        parameters.runtime.vol_min_observations,
    )?;
    insert_u64(
        &mut table,
        strategy_instance_id,
        "vol_bridge_valid_secs",
        parameters.runtime.vol_bridge_valid_secs,
    )?;
    insert_string(
        &mut table,
        "price_to_beat_source",
        parameters.runtime.price_to_beat_source.clone(),
    );
    insert_float(
        &mut table,
        "pricing_kurtosis",
        parameters.runtime.pricing_kurtosis,
    );
    insert_float(
        &mut table,
        "theta_decay_factor",
        parameters.runtime.theta_decay_factor,
    );
    insert_u64(
        &mut table,
        strategy_instance_id,
        "forced_flat_stale_reference_ms",
        parameters.runtime.forced_flat_stale_chainlink_ms,
    )?;
    insert_float(
        &mut table,
        "forced_flat_thin_book_min_liquidity",
        parameters.runtime.forced_flat_thin_book_min_liquidity,
    );
    insert_float(
        &mut table,
        "lead_agreement_min_corr",
        parameters.runtime.lead_agreement_min_corr,
    );
    insert_u64(
        &mut table,
        strategy_instance_id,
        "lead_jitter_max_ms",
        parameters.runtime.lead_jitter_max_ms,
    )?;

    Ok(Value::Table(table))
}

fn parameters_block(
    strategy: &LoadedStrategy,
) -> Result<ParametersBlock, BinaryOracleEdgeTakerRuntimeConfigError> {
    strategy
        .config
        .parameters
        .clone()
        .try_into()
        .map_err(
            |error| BinaryOracleEdgeTakerRuntimeConfigError::Parameters {
                strategy_instance_id: strategy.config.strategy_instance_id.clone(),
                message: error.to_string(),
            },
        )
}

fn nt_strategy_id(
    strategy: &LoadedStrategy,
) -> Result<String, BinaryOracleEdgeTakerRuntimeConfigError> {
    let mut value = strategy.config.strategy_archetype.as_str().to_string();
    value.push('-');
    value.push_str(&strategy.config.order_id_tag);
    StrategyId::new_checked(&value).map_err(|error| {
        BinaryOracleEdgeTakerRuntimeConfigError::StrategyId {
            strategy_instance_id: strategy.config.strategy_instance_id.clone(),
            value: value.clone(),
            message: error.to_string(),
        }
    })?;
    Ok(value)
}

fn binding_error(
    context: &StrategyRegistrationContext<'_>,
    error: BinaryOracleEdgeTakerRuntimeConfigError,
) -> BoltV3StrategyRegistrationError {
    binding_message(context, error.to_string())
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

fn decimal_string_to_f64(
    strategy_instance_id: &str,
    field: &'static str,
    value: &str,
) -> Result<f64, BinaryOracleEdgeTakerRuntimeConfigError> {
    let decimal = crate::bolt_v3_validate::parse_decimal_string(value).map_err(|_| {
        BinaryOracleEdgeTakerRuntimeConfigError::Numeric {
            strategy_instance_id: strategy_instance_id.to_string(),
            field,
            value: value.to_string(),
        }
    })?;
    decimal
        .to_f64()
        .ok_or_else(|| BinaryOracleEdgeTakerRuntimeConfigError::Numeric {
            strategy_instance_id: strategy_instance_id.to_string(),
            field,
            value: value.to_string(),
        })
}

fn i64_to_u64(
    strategy_instance_id: &str,
    field: &'static str,
    value: i64,
) -> Result<u64, BinaryOracleEdgeTakerRuntimeConfigError> {
    u64::try_from(value).map_err(|_| BinaryOracleEdgeTakerRuntimeConfigError::Numeric {
        strategy_instance_id: strategy_instance_id.to_string(),
        field,
        value: value.to_string(),
    })
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

fn insert_order_config(
    table: &mut Map<String, Value>,
    strategy_instance_id: &str,
    key: &'static str,
    order: &OrderParams,
) -> Result<(), BinaryOracleEdgeTakerRuntimeConfigError> {
    let mut order_table = Map::new();
    insert_string(&mut order_table, "side", enum_variant_lowercase(order.side));
    insert_string(
        &mut order_table,
        "position_side",
        enum_variant_lowercase(order.position_side),
    );
    insert_string(
        &mut order_table,
        "order_type",
        enum_variant_lowercase(order.order_type),
    );
    insert_string(
        &mut order_table,
        "time_in_force",
        enum_variant_lowercase(order.time_in_force),
    );
    if let Some(expire_time_unix_nanos) = order.expire_time_unix_nanos {
        insert_u64(
            &mut order_table,
            strategy_instance_id,
            "expire_time_unix_nanos",
            expire_time_unix_nanos,
        )?;
    }
    if let Some(trigger_price) = order.trigger_price {
        let trigger_price = trigger_price.to_f64().ok_or_else(|| {
            BinaryOracleEdgeTakerRuntimeConfigError::Numeric {
                strategy_instance_id: strategy_instance_id.to_string(),
                field: "trigger_price",
                value: trigger_price.to_string(),
            }
        })?;
        insert_float(&mut order_table, "trigger_price", trigger_price);
    }
    insert_bool(&mut order_table, "is_post_only", order.is_post_only);
    insert_bool(&mut order_table, "is_reduce_only", order.is_reduce_only);
    insert_bool(
        &mut order_table,
        "is_quote_quantity",
        order.is_quote_quantity,
    );
    table.insert(key.to_string(), Value::Table(order_table));
    Ok(())
}

fn enum_variant_lowercase<T: std::fmt::Display>(value: T) -> String {
    value.to_string().to_ascii_lowercase()
}

fn insert_i64(table: &mut Map<String, Value>, key: &'static str, value: i64) {
    table.insert(key.to_string(), Value::Integer(value));
}

fn insert_u64(
    table: &mut Map<String, Value>,
    strategy_instance_id: &str,
    key: &'static str,
    value: u64,
) -> Result<(), BinaryOracleEdgeTakerRuntimeConfigError> {
    let converted =
        i64::try_from(value).map_err(|_| BinaryOracleEdgeTakerRuntimeConfigError::Numeric {
            strategy_instance_id: strategy_instance_id.to_string(),
            field: key,
            value: value.to_string(),
        })?;
    table.insert(key.to_string(), Value::Integer(converted));
    Ok(())
}

fn insert_float(table: &mut Map<String, Value>, key: &'static str, value: f64) {
    table.insert(key.to_string(), Value::Float(value));
}

fn oms_type_value(strategy: &LoadedStrategy) -> String {
    enum_variant_lowercase(strategy.config.oms_type)
}

fn configured_reference_data(
    strategy: &LoadedStrategy,
) -> Result<&crate::bolt_v3_config::ReferenceDataBlock, BinaryOracleEdgeTakerRuntimeConfigError> {
    let mut entries = strategy.config.reference_data.iter();
    match (entries.next(), entries.next()) {
        (Some((_role, block)), None) => Ok(block),
        (None, _) => Err(BinaryOracleEdgeTakerRuntimeConfigError::ReferenceData {
            strategy_instance_id: strategy.config.strategy_instance_id.clone(),
            message: "requires exactly one [reference_data.<role>] block".to_string(),
        }),
        (Some(_), Some(_)) => Err(BinaryOracleEdgeTakerRuntimeConfigError::ReferenceData {
            strategy_instance_id: strategy.config.strategy_instance_id.clone(),
            message: format!(
                "requires exactly one [reference_data.<role>] block; got roles [{}]",
                reference_data_role_names(&strategy.config)
            ),
        }),
    }
}

fn reference_data_role_names(strategy: &BoltV3StrategyConfig) -> String {
    strategy
        .reference_data
        .keys()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(", ")
}

fn validate_required_reference_data(context: &str, strategy: &BoltV3StrategyConfig) -> Vec<String> {
    if strategy.reference_data.contains_key("primary") {
        Vec::new()
    } else {
        vec![format!(
            "{context}: strategy_archetype `binary_oracle_edge_taker` requires [reference_data.primary]"
        )]
    }
}

fn validate_order_parameters(
    context: &str,
    entry: &OrderParams,
    exit: &OrderParams,
) -> Vec<String> {
    let mut errors = Vec::new();
    errors.extend(check_strategy_position_contract(context, entry, exit));
    errors.extend(check_entry_order_combination(context, entry));
    errors.extend(check_exit_order_combination(context, exit));
    errors
}

fn validate_parameter_bounds(
    context: &str,
    parameters: &ParametersBlock,
    default_max_notional: Option<&Decimal>,
) -> Vec<String> {
    let mut errors = Vec::new();

    let order_target_decimal = match crate::bolt_v3_validate::parse_decimal_string(
        &parameters.order_notional_target,
    ) {
        Ok(value) => Some(value),
        Err(reason) => {
            errors.push(format!(
                    "{context}: parameters.order_notional_target is not a valid decimal string ({reason}): `{}`",
                    parameters.order_notional_target
                ));
            None
        }
    };
    if let Err(reason) =
        crate::bolt_v3_validate::parse_decimal_string(&parameters.maximum_position_notional)
    {
        errors.push(format!(
            "{context}: parameters.maximum_position_notional is not a valid decimal string ({reason}): `{}`",
            parameters.maximum_position_notional
        ));
    }
    if let (Some(order_target), Some(default_max)) =
        (order_target_decimal.as_ref(), default_max_notional)
        && order_target > default_max
    {
        errors.push(format!(
            "{context}: parameters.order_notional_target ({order_target}) must be <= root risk.default_max_notional_per_order ({default_max})"
        ));
    }

    errors
}

fn check_entry_order_combination(context: &str, entry: &OrderParams) -> Vec<String> {
    let taker_limit_fok = (OrderType::Limit, TimeInForce::Fok, false, false, false);
    let maker_limit_gtc = (OrderType::Limit, TimeInForce::Gtc, true, false, false);
    let gtd_limit = (OrderType::Limit, TimeInForce::Gtd, false, false);
    let stop_market = (OrderType::StopMarket, false, false, false);
    let market_if_touched = (OrderType::MarketIfTouched, false, false, false);
    let stop_limit = (OrderType::StopLimit, false, false);
    let limit_if_touched = (OrderType::LimitIfTouched, false, false);
    let actual = (
        entry.order_type,
        entry.time_in_force,
        entry.is_post_only,
        entry.is_reduce_only,
        entry.is_quote_quantity,
    );
    let is_taker_limit_fok = actual == taker_limit_fok && entry.trigger_price.is_none();
    let is_maker_limit_gtc = actual == maker_limit_gtc && entry.trigger_price.is_none();
    let is_gtd_limit_with_expiry = (
        entry.order_type,
        entry.time_in_force,
        entry.is_reduce_only,
        entry.is_quote_quantity,
    ) == gtd_limit
        && entry.expire_time_unix_nanos.is_some_and(|value| value > 0);
    let is_gtd_limit = is_gtd_limit_with_expiry && entry.trigger_price.is_none();
    let is_stop_market = (
        entry.order_type,
        entry.is_post_only,
        entry.is_reduce_only,
        entry.is_quote_quantity,
    ) == stop_market
        && has_positive_trigger_and_valid_expiry(entry);
    let is_market_if_touched = (
        entry.order_type,
        entry.is_post_only,
        entry.is_reduce_only,
        entry.is_quote_quantity,
    ) == market_if_touched
        && has_positive_trigger_and_valid_expiry(entry);
    let is_stop_limit = (
        entry.order_type,
        entry.is_reduce_only,
        entry.is_quote_quantity,
    ) == stop_limit
        && has_positive_trigger_and_valid_expiry(entry);
    let is_limit_if_touched = (
        entry.order_type,
        entry.is_reduce_only,
        entry.is_quote_quantity,
    ) == limit_if_touched
        && has_positive_trigger_and_valid_expiry(entry);
    if !is_taker_limit_fok
        && !is_maker_limit_gtc
        && !is_gtd_limit
        && !is_stop_market
        && !is_market_if_touched
        && !is_stop_limit
        && !is_limit_if_touched
    {
        vec![format!(
            "{context}: parameters.entry_order combination is not allowed for `binary_oracle_edge_taker`; \
             only order_type=limit with time_in_force=fok, time_in_force=gtc plus is_post_only=true, time_in_force=gtd plus expire_time_unix_nanos, order_type=stop_market plus trigger_price, order_type=market_if_touched plus trigger_price, order_type=stop_limit plus trigger_price, or order_type=limit_if_touched plus trigger_price is allowed; \
             is_reduce_only=false and is_quote_quantity=false are required"
        )]
    } else {
        Vec::new()
    }
}

fn check_exit_order_combination(context: &str, exit: &OrderParams) -> Vec<String> {
    let taker_market_ioc = (OrderType::Market, TimeInForce::Ioc, false, false, false);
    let maker_limit_gtc = (OrderType::Limit, TimeInForce::Gtc, true, false, false);
    let gtd_limit = (OrderType::Limit, TimeInForce::Gtd, false, false);
    let stop_market = (OrderType::StopMarket, false, false, false);
    let stop_limit = (OrderType::StopLimit, false, false);
    let limit_if_touched = (OrderType::LimitIfTouched, false, false);
    let actual = (
        exit.order_type,
        exit.time_in_force,
        exit.is_post_only,
        exit.is_reduce_only,
        exit.is_quote_quantity,
    );
    let is_taker_market_ioc = actual == taker_market_ioc && exit.trigger_price.is_none();
    let is_maker_limit_gtc = actual == maker_limit_gtc && exit.trigger_price.is_none();
    let is_gtd_limit_with_expiry = (
        exit.order_type,
        exit.time_in_force,
        exit.is_reduce_only,
        exit.is_quote_quantity,
    ) == gtd_limit
        && exit.expire_time_unix_nanos.is_some_and(|value| value > 0);
    let is_gtd_limit = is_gtd_limit_with_expiry && exit.trigger_price.is_none();
    let is_stop_market = (
        exit.order_type,
        exit.is_post_only,
        exit.is_reduce_only,
        exit.is_quote_quantity,
    ) == stop_market
        && has_positive_trigger_and_valid_expiry(exit);
    let is_stop_limit = (exit.order_type, exit.is_reduce_only, exit.is_quote_quantity)
        == stop_limit
        && has_positive_trigger_and_valid_expiry(exit);
    let is_limit_if_touched = (exit.order_type, exit.is_reduce_only, exit.is_quote_quantity)
        == limit_if_touched
        && has_positive_trigger_and_valid_expiry(exit);
    if !is_taker_market_ioc
        && !is_maker_limit_gtc
        && !is_gtd_limit
        && !is_stop_market
        && !is_stop_limit
        && !is_limit_if_touched
    {
        vec![format!(
            "{context}: parameters.exit_order combination is not allowed for `binary_oracle_edge_taker`; \
             only order_type=market with time_in_force=ioc, order_type=limit with time_in_force=gtc plus is_post_only=true, order_type=limit with time_in_force=gtd plus expire_time_unix_nanos, order_type=stop_market plus trigger_price, order_type=stop_limit plus trigger_price, or order_type=limit_if_touched plus trigger_price is allowed; \
             is_reduce_only=false and is_quote_quantity=false are required"
        )]
    } else {
        Vec::new()
    }
}

fn has_positive_trigger_and_valid_expiry(order: &OrderParams) -> bool {
    order
        .trigger_price
        .is_some_and(|value| value > Decimal::ZERO)
        && (order.time_in_force != TimeInForce::Gtd
            || order.expire_time_unix_nanos.is_some_and(|value| value > 0))
}

fn check_strategy_position_contract(
    context: &str,
    entry: &OrderParams,
    exit: &OrderParams,
) -> Vec<String> {
    if expected_position_side_for_entry_order(entry.side)
        .is_some_and(|side| side == entry.position_side)
        && expected_exit_order_side_for_position(exit.position_side)
            .is_some_and(|side| side == exit.side)
        && entry.position_side == exit.position_side
        && is_observed_open_side(entry.position_side)
    {
        Vec::new()
    } else {
        vec![format!(
            "{context}: parameters entry/exit order position contract is not supported for `binary_oracle_edge_taker`; \
             long requires entry side=buy, exit side=sell, position_side=long; \
             short requires entry side=sell, exit side=buy, position_side=short"
        )]
    }
}

fn expected_position_side_for_entry_order(order_side: OrderSide) -> Option<PositionSide> {
    match order_side {
        OrderSide::Buy => Some(PositionSide::Long),
        OrderSide::Sell => Some(PositionSide::Short),
        _ => None,
    }
}

fn expected_exit_order_side_for_position(position_side: PositionSide) -> Option<OrderSide> {
    match position_side {
        PositionSide::Long => Some(OrderSide::Sell),
        PositionSide::Short => Some(OrderSide::Buy),
        _ => None,
    }
}

fn is_observed_open_side(side: PositionSide) -> bool {
    matches!(side, PositionSide::Long | PositionSide::Short)
}
