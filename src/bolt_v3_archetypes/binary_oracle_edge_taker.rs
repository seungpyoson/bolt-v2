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
//! 2. The archetype's bolt-v3 startup-validation policy: exactly one
//!    configured reference-data role. Entry and exit order shape values
//!    are TOML-owned and projected into the runtime strategy config
//!    instead of being narrowed by a code-owned combination gate.
//!
//! Core startup validation in `crate::bolt_v3_validate` keeps target-
//! shape and per-role reference-data structural checks structural and
//! dispatches archetype-specific rules through
//! `crate::bolt_v3_archetypes::validate_strategy_archetype` based on
//! `strategy.strategy_archetype`. Archetype-specific error-message
//! policy lives here so that a future archetype can introduce its own
//! message contract without reaching back into core validation.

use std::sync::Arc;

use rust_decimal::{Decimal, prelude::ToPrimitive};
use serde::Deserialize;
use toml::{Value, map::Map};

use nautilus_model::identifiers::StrategyId;

use crate::{
    bolt_v3_adapters::BoltV3AdapterMappingError,
    bolt_v3_archetypes::ArchetypeValidationBinding,
    bolt_v3_config::{
        BoltV3StrategyConfig, LoadedBoltV3Config, LoadedStrategy, ReferenceDataBlock, VenueBlock,
    },
    bolt_v3_market_families, bolt_v3_providers,
    bolt_v3_secrets::ResolvedBoltV3Secrets,
    bolt_v3_strategy_registration::{
        BoltV3StrategyRegistrationError, StrategyRegistrationContext, StrategyRuntimeBinding,
    },
    strategies::{
        binary_oracle_edge_taker::{BinaryOracleEdgeTakerBuilder, KEY as STRATEGY_KIND},
        production_strategy_registry,
        registry::{FeeProvider, StrategyBuildContext, StrategyBuilder},
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
    strategy_kind,
    register: register_runtime_strategy,
};

fn strategy_kind() -> &'static str {
    BinaryOracleEdgeTakerBuilder::kind()
}

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
    pub forced_flat_stale_reference_ms: u64,
    pub forced_flat_thin_book_min_liquidity: f64,
    pub lead_agreement_min_corr: f64,
    pub lead_jitter_max_ms: u64,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct OrderParams {
    pub side: ArchetypeOrderSide,
    pub position_side: ArchetypePositionSide,
    pub order_type: ArchetypeOrderType,
    pub time_in_force: ArchetypeTimeInForce,
    pub is_post_only: bool,
    pub is_reduce_only: bool,
    pub is_quote_quantity: bool,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ArchetypeOrderSide {
    Buy,
    Sell,
}

impl ArchetypeOrderSide {
    fn as_str(self) -> String {
        match self {
            Self::Buy => "buy",
            Self::Sell => "sell",
        }
        .to_string()
    }
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ArchetypePositionSide {
    Long,
    Short,
}

impl ArchetypePositionSide {
    fn as_str(self) -> String {
        match self {
            Self::Long => "long",
            Self::Short => "short",
        }
        .to_string()
    }
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ArchetypeOrderType {
    Limit,
    Market,
}

impl ArchetypeOrderType {
    fn as_str(self) -> String {
        match self {
            Self::Limit => "limit",
            Self::Market => "market",
        }
        .to_string()
    }
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ArchetypeTimeInForce {
    Gtc,
    Fok,
    Ioc,
}

impl ArchetypeTimeInForce {
    fn as_str(self) -> String {
        match self {
            Self::Gtc => "gtc",
            Self::Fok => "fok",
            Self::Ioc => "ioc",
        }
        .to_string()
    }
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

    errors.extend(validate_parameter_bounds(
        context,
        &parameters,
        default_max_notional,
    ));
    errors.extend(validate_strategy_runtime_fields(context, strategy));
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
    Venue {
        strategy_instance_id: String,
        venue: String,
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
            Self::Venue {
                strategy_instance_id,
                venue,
            } => write!(
                f,
                "strategies.{strategy_instance_id} venue `{venue}` is not present in loaded venues"
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
    let venue = context
        .loaded
        .root
        .venues
        .get(&context.strategy.config.venue)
        .ok_or_else(|| {
            binding_message(
                &context,
                format!(
                    "strategy venue `{}` is not present in loaded venues",
                    context.strategy.config.venue
                ),
            )
        })?;
    let fee_provider = build_fee_provider_for_strategy_venue(
        &context.strategy.config.venue,
        venue,
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
            context.strategy_kind,
            &raw,
            &build_context,
            node.kernel().trader(),
        )
        .map_err(|error| binding_message(&context, error.to_string()))
}

fn build_fee_provider_for_strategy_venue(
    venue_key: &str,
    venue: &VenueBlock,
    resolved: &ResolvedBoltV3Secrets,
) -> Result<Arc<dyn FeeProvider>, BoltV3AdapterMappingError> {
    let binding =
        bolt_v3_providers::binding_for_provider_key(venue.kind.as_str()).ok_or_else(|| {
            BoltV3AdapterMappingError::ValidationInvariant {
                venue_key: venue_key.to_string(),
                field: "kind",
                message: format!(
                    "provider kind `{}` is not supported by this build",
                    venue.kind.as_str()
                ),
            }
        })?;
    let build_fee_provider = binding.build_fee_provider.ok_or_else(|| {
        BoltV3AdapterMappingError::ValidationInvariant {
            venue_key: venue_key.to_string(),
            field: "kind",
            message: format!(
                "provider kind `{}` does not provide taker fees for strategy_archetype `{KEY}`",
                venue.kind.as_str()
            ),
        }
    })?;

    build_fee_provider(venue_key, venue, resolved)
}

pub fn raw_taker_config(
    strategy: &LoadedStrategy,
    loaded: &LoadedBoltV3Config,
) -> Result<Value, BinaryOracleEdgeTakerRuntimeConfigError> {
    if strategy.config.strategy_archetype.as_str() != KEY {
        return Err(BinaryOracleEdgeTakerRuntimeConfigError::WrongArchetype {
            expected: KEY,
            actual: strategy.config.strategy_archetype.as_str().to_string(),
        });
    }

    let parameters = parameters_block(strategy)?;
    let target =
        bolt_v3_market_families::target_runtime_fields_from_target(&strategy.config.target)
            .map_err(|message| BinaryOracleEdgeTakerRuntimeConfigError::Target {
                strategy_instance_id: strategy.config.strategy_instance_id.clone(),
                message,
            })?;
    loaded
        .root
        .venues
        .get(&strategy.config.venue)
        .ok_or_else(|| BinaryOracleEdgeTakerRuntimeConfigError::Venue {
            strategy_instance_id: strategy.config.strategy_instance_id.clone(),
            venue: strategy.config.venue.clone(),
        })?;
    let reference_data = configured_reference_data(strategy)?;

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
    insert_string(&mut table, "oms_type", oms_type_value(strategy).to_string());
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
    insert_string(&mut table, "client_id", strategy.config.venue.clone());
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
    insert_string(&mut table, "reference_venue", reference_data.venue.clone());
    insert_string(
        &mut table,
        "reference_instrument_id",
        reference_data.instrument_id.clone(),
    );
    insert_order_config(&mut table, "entry_order", &parameters.entry_order);
    insert_order_config(&mut table, "exit_order", &parameters.exit_order);
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
        parameters.runtime.forced_flat_stale_reference_ms,
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

fn configured_reference_data(
    strategy: &LoadedStrategy,
) -> Result<&ReferenceDataBlock, BinaryOracleEdgeTakerRuntimeConfigError> {
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

fn oms_type_value(strategy: &LoadedStrategy) -> &'static str {
    match strategy.config.oms_type {
        crate::bolt_v3_config::OmsType::Netting => "netting",
    }
}

fn insert_string(table: &mut Map<String, Value>, key: &'static str, value: String) {
    table.insert(key.to_string(), Value::String(value));
}

fn insert_order_config(table: &mut Map<String, Value>, key: &'static str, order: &OrderParams) {
    let mut order_table = Map::new();
    insert_string(&mut order_table, "side", order.side.as_str());
    insert_string(
        &mut order_table,
        "position_side",
        order.position_side.as_str(),
    );
    insert_string(&mut order_table, "order_type", order.order_type.as_str());
    insert_string(
        &mut order_table,
        "time_in_force",
        order.time_in_force.as_str(),
    );
    insert_bool(&mut order_table, "is_post_only", order.is_post_only);
    insert_bool(&mut order_table, "is_reduce_only", order.is_reduce_only);
    insert_bool(
        &mut order_table,
        "is_quote_quantity",
        order.is_quote_quantity,
    );
    table.insert(key.to_string(), Value::Table(order_table));
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

fn validate_required_reference_data(context: &str, strategy: &BoltV3StrategyConfig) -> Vec<String> {
    match strategy.reference_data.len() {
        1 => Vec::new(),
        0 => vec![format!(
            "{context}: strategy_archetype `binary_oracle_edge_taker` requires exactly one [reference_data.<role>] block"
        )],
        _ => vec![format!(
            "{context}: strategy_archetype `binary_oracle_edge_taker` requires exactly one [reference_data.<role>] block; got roles [{}]",
            reference_data_role_names(strategy)
        )],
    }
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

fn validate_strategy_runtime_fields(context: &str, strategy: &BoltV3StrategyConfig) -> Vec<String> {
    if toml::Value::String(strategy.market_exit_time_in_force.clone())
        .try_into::<ArchetypeTimeInForce>()
        .is_ok()
    {
        return Vec::new();
    }

    vec![format!(
        "{context}: market_exit_time_in_force must be a supported NT TimeInForce value for binary_oracle_edge_taker: `{}`",
        strategy.market_exit_time_in_force
    )]
}
