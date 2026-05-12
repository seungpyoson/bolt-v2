//! Strategy-archetype binding for `binary_oracle_edge_taker`.
//!
//! This module owns:
//!
//! 1. The archetype's `[parameters]` block shape (`ParametersBlock`),
//!    its `[parameters.entry_order]` / `[parameters.exit_order]` row
//!    shape (`OrderParams`), and the order-type / time-in-force enums
//!    those rows reference (`ArchetypeOrderType`,
//!    `ArchetypeTimeInForce`). Core config in
//!    `crate::bolt_v3_config` keeps the strategy envelope and the
//!    field name `parameters`, but the row shape and enum values are
//!    archetype-specific and live here so a future archetype can
//!    introduce its own parameter row without reaching back into core
//!    config.
//! 2. The archetype's bolt-v3 startup-validation policy:
//!    - the required reference-data capabilities
//!      (one oracle-capable role and one orderbook-capable role),
//!    - the allowed `[parameters.entry_order]` combination
//!      (`order_type=limit`, `time_in_force=fok`, all boolean flags
//!      `false`),
//!    - the allowed `[parameters.exit_order]` combination
//!      (`order_type=market`, `time_in_force=ioc`, all boolean flags
//!      `false`).
//!
//! Core startup validation in `crate::bolt_v3_validate` keeps target-
//! shape and per-role reference-data structural checks structural and
//! dispatches archetype-specific rules through
//! `crate::bolt_v3_archetypes::validate_strategy_archetype` based on
//! `strategy.strategy_archetype`. Archetype-specific error-message
//! policy (the headline "is not allowed for `binary_oracle_edge_taker`"
//! phrase and the per-field rule listing) lives here so that a
//! future archetype can introduce its own message contract without
//! reaching back into core validation.

use std::sync::Arc;

use nautilus_live::node::LiveNode;
use nautilus_model::identifiers::StrategyId;
use rust_decimal::{Decimal, prelude::ToPrimitive};
use serde::Deserialize;
use toml::{Value, map::Map};

use crate::{
    bolt_v3_archetypes::{ArchetypeValidationBinding, ReferenceCapabilityRequirement},
    bolt_v3_config::{BoltV3StrategyConfig, LoadedStrategy},
    bolt_v3_market_families::updown::TargetBlock,
    bolt_v3_providers::{ReferenceCapability, polymarket},
    bolt_v3_strategy_registration::{
        BoltV3StrategyRegistrationError, StrategyRegistrationContext, StrategyRuntimeBinding,
    },
    strategies::{
        eth_chainlink_taker::EthChainlinkTakerBuilder,
        production_strategy_registry,
        registry::{StrategyBuildContext, StrategyBuilder},
    },
};

pub const KEY: &str = "binary_oracle_edge_taker";
pub const REFERENCE_CAPABILITY_REQUIREMENTS: &[ReferenceCapabilityRequirement] = &[
    ReferenceCapabilityRequirement {
        capability: ReferenceCapability::Oracle,
        minimum_count: 1,
        description: "oracle-capable reference_data role",
    },
    ReferenceCapabilityRequirement {
        capability: ReferenceCapability::Orderbook,
        minimum_count: 1,
        description: "orderbook-capable reference_data role",
    },
];

pub fn validation_binding() -> ArchetypeValidationBinding {
    ArchetypeValidationBinding {
        key: KEY,
        validate_strategy,
        reference_capability_requirements: REFERENCE_CAPABILITY_REQUIREMENTS,
    }
}

pub const RUNTIME_BINDING: StrategyRuntimeBinding = StrategyRuntimeBinding {
    key: KEY,
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
    pub order_type: ArchetypeOrderType,
    pub time_in_force: ArchetypeTimeInForce,
    pub is_post_only: bool,
    pub is_reduce_only: bool,
    pub is_quote_quantity: bool,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ArchetypeOrderType {
    Limit,
    Market,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ArchetypeTimeInForce {
    Gtc,
    Fok,
    Ioc,
}

pub fn validate_strategy(
    context: &str,
    strategy: &BoltV3StrategyConfig,
    default_max_notional: Option<&Decimal>,
) -> Vec<String> {
    let mut errors = Vec::new();

    if let Some(parameters) = strategy.parameters.as_table()
        && !parameters.contains_key("runtime")
    {
        errors.push(format!(
            "{context}: parameters.runtime is required for `binary_oracle_edge_taker`"
        ));
        return errors;
    }

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
    node: &mut LiveNode,
    context: StrategyRegistrationContext<'_>,
) -> Result<StrategyId, BoltV3StrategyRegistrationError> {
    let raw = raw_taker_config(context.strategy).map_err(|error| binding_error(context, error))?;
    let parameters =
        parameters_block(context.strategy).map_err(|error| binding_error(context, error))?;
    let venue = context
        .loaded
        .root
        .venues
        .get(&context.strategy.config.venue)
        .ok_or_else(|| {
            binding_message(
                context,
                format!(
                    "strategy venue `{}` is not present in loaded venues",
                    context.strategy.config.venue
                ),
            )
        })?;
    let fee_provider =
        polymarket::build_fee_provider(&context.strategy.config.venue, venue, context.resolved)
            .map_err(|error| binding_message(context, error.to_string()))?;
    let decision_evidence = Arc::new(
        crate::bolt_v3_decision_evidence::JsonlBoltV3DecisionEvidenceWriter::from_loaded_config(
            context.loaded,
        )
        .map_err(|error| binding_message(context, error.to_string()))?,
    );
    let build_context = StrategyBuildContext::try_new(
        fee_provider,
        parameters.runtime.reference_publish_topic,
        Some(decision_evidence),
    )
    .map_err(|error| binding_message(context, error.to_string()))?;
    let registry = production_strategy_registry()
        .map_err(|error| binding_message(context, error.to_string()))?;
    registry
        .register_strategy(
            EthChainlinkTakerBuilder::kind(),
            &raw,
            &build_context,
            node.kernel().trader(),
        )
        .map_err(|error| binding_message(context, error.to_string()))
}

pub fn raw_taker_config(
    strategy: &LoadedStrategy,
) -> Result<Value, BinaryOracleEdgeTakerRuntimeConfigError> {
    if strategy.config.strategy_archetype.as_str() != KEY {
        return Err(BinaryOracleEdgeTakerRuntimeConfigError::WrongArchetype {
            expected: KEY,
            actual: strategy.config.strategy_archetype.as_str().to_string(),
        });
    }

    let parameters = parameters_block(strategy)?;
    let target: TargetBlock = strategy.config.target.clone().try_into().map_err(|error| {
        BinaryOracleEdgeTakerRuntimeConfigError::Target {
            strategy_instance_id: strategy.config.strategy_instance_id.clone(),
            message: error.to_string(),
        }
    })?;

    let max_position_usdc = decimal_string_to_f64(
        &strategy.config.strategy_instance_id,
        "parameters.maximum_position_notional",
        &parameters.maximum_position_notional,
    )?;
    let period_duration_secs = i64_to_u64(
        &strategy.config.strategy_instance_id,
        "target.cadence_seconds",
        target.cadence_seconds,
    )?;

    let mut table = Map::new();
    insert_string(&mut table, "strategy_id", nt_strategy_id(strategy)?);
    insert_string(&mut table, "client_id", strategy.config.venue.clone());
    insert_u64(
        &mut table,
        "warmup_tick_count",
        parameters.runtime.warmup_tick_count,
    );
    insert_u64(&mut table, "period_duration_secs", period_duration_secs);
    insert_u64(
        &mut table,
        "reentry_cooldown_secs",
        parameters.runtime.reentry_cooldown_secs,
    );
    insert_float(&mut table, "max_position_usdc", max_position_usdc);
    insert_u64(
        &mut table,
        "book_impact_cap_bps",
        parameters.runtime.book_impact_cap_bps,
    );
    insert_float(&mut table, "risk_lambda", parameters.runtime.risk_lambda);
    insert_i64(
        &mut table,
        "worst_case_ev_min_bps",
        parameters.edge_threshold_basis_points,
    );
    insert_i64(
        &mut table,
        "exit_hysteresis_bps",
        parameters.runtime.exit_hysteresis_bps,
    );
    insert_u64(
        &mut table,
        "vol_window_secs",
        parameters.runtime.vol_window_secs,
    );
    insert_u64(
        &mut table,
        "vol_gap_reset_secs",
        parameters.runtime.vol_gap_reset_secs,
    );
    insert_u64(
        &mut table,
        "vol_min_observations",
        parameters.runtime.vol_min_observations,
    );
    insert_u64(
        &mut table,
        "vol_bridge_valid_secs",
        parameters.runtime.vol_bridge_valid_secs,
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
        "forced_flat_stale_chainlink_ms",
        parameters.runtime.forced_flat_stale_chainlink_ms,
    );
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
        "lead_jitter_max_ms",
        parameters.runtime.lead_jitter_max_ms,
    );

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
    context: StrategyRegistrationContext<'_>,
    error: BinaryOracleEdgeTakerRuntimeConfigError,
) -> BoltV3StrategyRegistrationError {
    binding_message(context, error.to_string())
}

fn binding_message(
    context: StrategyRegistrationContext<'_>,
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

fn insert_i64(table: &mut Map<String, Value>, key: &'static str, value: i64) {
    table.insert(key.to_string(), Value::Integer(value));
}

fn insert_u64(table: &mut Map<String, Value>, key: &'static str, value: u64) {
    let converted = i64::try_from(value).expect("validated runtime integer must fit in toml value");
    table.insert(key.to_string(), Value::Integer(converted));
}

fn insert_float(table: &mut Map<String, Value>, key: &'static str, value: f64) {
    table.insert(key.to_string(), Value::Float(value));
}

fn validate_order_parameters(
    context: &str,
    entry: &OrderParams,
    exit: &OrderParams,
) -> Vec<String> {
    let mut errors = Vec::new();
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
    let expected = (
        ArchetypeOrderType::Limit,
        ArchetypeTimeInForce::Fok,
        false,
        false,
        false,
    );
    let actual = (
        entry.order_type,
        entry.time_in_force,
        entry.is_post_only,
        entry.is_reduce_only,
        entry.is_quote_quantity,
    );
    if actual != expected {
        vec![format!(
            "{context}: parameters.entry_order combination is not allowed for `binary_oracle_edge_taker`; \
             only order_type=limit, time_in_force=fok, is_post_only=false, is_reduce_only=false, is_quote_quantity=false is allowed"
        )]
    } else {
        Vec::new()
    }
}

fn check_exit_order_combination(context: &str, exit: &OrderParams) -> Vec<String> {
    let expected = (
        ArchetypeOrderType::Market,
        ArchetypeTimeInForce::Ioc,
        false,
        false,
        false,
    );
    let actual = (
        exit.order_type,
        exit.time_in_force,
        exit.is_post_only,
        exit.is_reduce_only,
        exit.is_quote_quantity,
    );
    if actual != expected {
        vec![format!(
            "{context}: parameters.exit_order combination is not allowed for `binary_oracle_edge_taker`; \
             only order_type=market, time_in_force=ioc, is_post_only=false, is_reduce_only=false, is_quote_quantity=false is allowed"
        )]
    } else {
        Vec::new()
    }
}
