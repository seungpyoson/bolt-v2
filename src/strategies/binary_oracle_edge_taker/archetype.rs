//! Strategy-archetype binding for `binary_oracle_edge_taker`.
//!
//! This module owns:
//!
//! 1. The archetype's `[parameters]` block shape (`ParametersBlock`)
//!    and its `[parameters.entry_order]` / `[parameters.exit_order]` /
//!    `[parameters.forced_exit_order]` row shape (`OrderParams`).
//!    The `order_type` and `time_in_force`
//!    fields on `OrderParams` are typed with NT's canonical
//!    `nautilus_model::enums::{OrderType, TimeInForce}`; this archetype's
//!    validator checks enabled NT order-template invariants rather than
//!    defining a narrower shadow enum. Core config in
//!    `crate::bolt_v3_config` keeps the strategy envelope and the
//!    field name `parameters`, but the row shape is archetype-specific
//!    and lives here so a future archetype can introduce its own
//!    parameter row without reaching back into core config.
//! 2. The archetype's bolt-v3 startup-validation policy:
//!    - the required `[reference_current_price]` source set,
//!    - the enabled `[parameters.entry_order]` and
//!      `[parameters.exit_order]` and `[parameters.forced_exit_order]`
//!      NT order-template invariants, including required GTD expiry,
//!      triggered-order trigger fields, and trailing-stop fields.
//!
//! Core startup validation in `crate::bolt_v3_validate` keeps target and
//! market-data structural checks and dispatches archetype-specific rules through
//! `crate::bolt_v3_archetypes::validate_strategy_archetype_with_bindings`
//! based on `strategy.strategy_archetype`. Archetype-specific error-message
//! policy (the headline "is not allowed for `binary_oracle_edge_taker`"
//! phrase, the per-field rule listing, and the
//! market-data structural wording) lives here so that a future archetype can
//! introduce its own message contract without reaching back into core
//! validation.

use std::collections::BTreeSet;

use rust_decimal::{Decimal, prelude::ToPrimitive};
use serde::{Deserialize, Deserializer};
use toml::{Value, map::Map};

use nautilus_model::{
    enums::{OrderSide, OrderType, PositionSide, TimeInForce, TrailingOffsetType, TriggerType},
    identifiers::{InstrumentId, StrategyId, Venue},
};

use crate::{
    bolt_v3_archetypes::{
        ArchetypeGateRequirement, ArchetypeValidationBinding, GateRole, GateValueKind,
    },
    bolt_v3_config::{
        BoltV3RootConfig, BoltV3StrategyConfig, DECISION_REFERENCE_GATE_ROLE, LoadedStrategy,
        RESOLUTION_GATE_ROLE,
    },
    bolt_v3_numeric::MILLIS_PER_SECOND_U64,
    bolt_v3_order_intent::{NtOrderTemplateConfig, check_nt_order_template_config},
    bolt_v3_position_contract::{
        expected_exit_order_side_for_position, expected_position_side_for_entry_order,
        is_observed_open_side,
    },
    bolt_v3_providers::{
        ProviderMarketExitOrderConstraints, binding_for_provider_key,
        resolution_oracle_client_http_timeout_secs,
    },
    bolt_v3_strategy_registration::{
        BoltV3StrategyRegistrationError, PreparedStrategyClientRoutes,
        PreparedStrategyRegistration, StrategyPreparationConfig, StrategyRegistrationContext,
        StrategyRuntimeBinding, StrategyRuntimeCapabilities, assemble_strategy_build_context,
        execution_account_id, settlement_currency_for_execution_account, venue_for_client,
    },
    strategies::{
        binary_oracle_edge_taker::{BinaryOracleEdgeTakerBuilder, KEY as STRATEGY_KIND},
        production_strategy_registry,
        registry::StrategyBuilder,
    },
};

pub const KEY: &str = STRATEGY_KIND;
pub const BINARY_ORACLE_ENTRY_ORDER_UNSUPPORTED_SHAPE_CODE: &str =
    "binary_oracle_entry_order_unsupported_shape";
pub const BINARY_ORACLE_ENTRY_ORDER_REDUCE_ONLY_CODE: &str =
    "binary_oracle_entry_order_reduce_only";

pub fn validation_binding() -> ArchetypeValidationBinding {
    ArchetypeValidationBinding {
        key: KEY,
        validate_strategy,
    }
}

pub const RUNTIME_BINDING: StrategyRuntimeBinding = StrategyRuntimeBinding {
    key: KEY,
    strategy_kind: KEY,
    capabilities: StrategyRuntimeCapabilities {
        realized_volatility: true,
        settlement: true,
    },
    prepare: prepare_runtime_strategy,
};

pub fn gate_requirements() -> Vec<ArchetypeGateRequirement> {
    vec![ArchetypeGateRequirement {
        role: GateRole::Resolution,
        required: true,
        accepted_value_kinds: BTreeSet::from([GateValueKind::Price, GateValueKind::Outcome]),
        allow_no_resolution: false,
    }]
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
    pub forced_exit_order: OrderParams,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RuntimeParametersBlock {
    pub reference_publish_topic: String,
    pub warmup_tick_count: u64,
    pub reentry_cooldown_secs: u64,
    pub book_impact_cap_bps: u64,
    pub vwap_depth_limit_bps: u64,
    pub slippage_buffer_bps: u64,
    pub risk_lambda: f64,
    pub sizing_ev_reference_bps: u64,
    pub exit_hysteresis_bps: i64,
    pub trade_flow_window_secs: u64,
    pub trade_flow_max_samples: u64,
    pub spike_guard_return_threshold: f64,
    pub spike_guard_cooldown_secs: u64,
    pub pricing_kurtosis: f64,
    pub theta_decay_factor: f64,
    pub forced_flat_thin_book_min_liquidity: f64,
    pub lead_agreement_min_corr: f64,
    pub lead_jitter_max_ms: u64,
}

impl<'de> Deserialize<'de> for RuntimeParametersBlock {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            reference_publish_topic: String,
            warmup_tick_count: u64,
            reentry_cooldown_secs: u64,
            book_impact_cap_bps: u64,
            vwap_depth_limit_bps: u64,
            slippage_buffer_bps: u64,
            risk_lambda: f64,
            sizing_ev_reference_bps: u64,
            exit_hysteresis_bps: i64,
            trade_flow_window_secs: u64,
            trade_flow_max_samples: u64,
            spike_guard_return_threshold: f64,
            spike_guard_cooldown_secs: u64,
            pricing_kurtosis: f64,
            theta_decay_factor: f64,
            forced_flat_thin_book_min_liquidity: f64,
            lead_agreement_min_corr: f64,
            lead_jitter_max_ms: u64,
            price_to_beat_source: Option<toml::Value>,
            price_to_beat_feed_id: Option<toml::Value>,
            price_to_beat_report_schema_version: Option<toml::Value>,
            price_to_beat_report_decimal_scale: Option<toml::Value>,
            forced_flat_stale_chainlink_ms: Option<toml::Value>,
            chainlink_data_streams_feed_id: Option<toml::Value>,
        }

        let wire = Wire::deserialize(deserializer)?;
        if wire.price_to_beat_source.is_some() {
            return Err(serde::de::Error::custom(
                "parameters.runtime.price_to_beat_source must move to [target.gate_subscriptions.<role>]",
            ));
        }
        if wire.price_to_beat_feed_id.is_some() {
            return Err(serde::de::Error::custom(
                "parameters.runtime.price_to_beat_feed_id must move to [gate_providers.<id>.<provider_kind>]",
            ));
        }
        if wire.price_to_beat_report_schema_version.is_some() {
            return Err(serde::de::Error::custom(
                "parameters.runtime.price_to_beat_report_schema_version must move to [gate_providers.<id>.<provider_kind>]",
            ));
        }
        if wire.price_to_beat_report_decimal_scale.is_some() {
            return Err(serde::de::Error::custom(
                "parameters.runtime.price_to_beat_report_decimal_scale must move to [gate_providers.<id>.<provider_kind>]",
            ));
        }
        if wire.forced_flat_stale_chainlink_ms.is_some() {
            return Err(serde::de::Error::custom(
                "parameters.runtime.forced_flat_stale_chainlink_ms must move to [gate_providers.<id>.freshness]",
            ));
        }
        if wire.chainlink_data_streams_feed_id.is_some() {
            return Err(serde::de::Error::custom(
                "parameters.runtime.chainlink_data_streams_feed_id must move to [gate_providers.<id>.chainlink_data_streams]",
            ));
        }

        Ok(Self {
            reference_publish_topic: wire.reference_publish_topic,
            warmup_tick_count: wire.warmup_tick_count,
            reentry_cooldown_secs: wire.reentry_cooldown_secs,
            book_impact_cap_bps: wire.book_impact_cap_bps,
            vwap_depth_limit_bps: wire.vwap_depth_limit_bps,
            slippage_buffer_bps: wire.slippage_buffer_bps,
            risk_lambda: wire.risk_lambda,
            sizing_ev_reference_bps: wire.sizing_ev_reference_bps,
            exit_hysteresis_bps: wire.exit_hysteresis_bps,
            trade_flow_window_secs: wire.trade_flow_window_secs,
            trade_flow_max_samples: wire.trade_flow_max_samples,
            spike_guard_return_threshold: wire.spike_guard_return_threshold,
            spike_guard_cooldown_secs: wire.spike_guard_cooldown_secs,
            pricing_kurtosis: wire.pricing_kurtosis,
            theta_decay_factor: wire.theta_decay_factor,
            forced_flat_thin_book_min_liquidity: wire.forced_flat_thin_book_min_liquidity,
            lead_agreement_min_corr: wire.lead_agreement_min_corr,
            lead_jitter_max_ms: wire.lead_jitter_max_ms,
        })
    }
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
    pub activation_price: Option<Decimal>,
    pub trigger_type: Option<TriggerType>,
    pub trigger_instrument_id: Option<InstrumentId>,
    pub trailing_offset: Option<Decimal>,
    pub trailing_offset_type: Option<TrailingOffsetType>,
    pub is_post_only: bool,
    pub is_reduce_only: bool,
    pub is_quote_quantity: bool,
}

impl OrderParams {
    fn nt_order_template_config(&self) -> NtOrderTemplateConfig {
        NtOrderTemplateConfig {
            order_type: self.order_type,
            time_in_force: self.time_in_force,
            expire_time_unix_nanos: self.expire_time_unix_nanos,
            trigger_price: self.trigger_price,
            activation_price: self.activation_price,
            trigger_type: self.trigger_type,
            trigger_instrument_id: self.trigger_instrument_id,
            trailing_offset: self.trailing_offset,
            trailing_offset_type: self.trailing_offset_type,
            is_post_only: self.is_post_only,
            is_reduce_only: self.is_reduce_only,
            is_quote_quantity: self.is_quote_quantity,
        }
    }
}

pub fn validate_strategy(
    context: &str,
    root: &BoltV3RootConfig,
    strategy: &BoltV3StrategyConfig,
    default_max_notional: Option<&Decimal>,
) -> Vec<String> {
    let mut errors = validate_required_market_data(context, strategy);
    errors.extend(validate_reference_current_price_forced_flat_grace(
        context, root, strategy,
    ));
    errors.extend(validate_resolution_retry_interval_covers_http_timeout(
        context, root, strategy,
    ));
    if strategy.realized_volatility_surface_id.is_none() {
        errors.push(format!(
            "{context}: realized_volatility_surface_id is required"
        ));
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

    let market_exit_order_constraints =
        strategy_execution_client_market_exit_order_constraints(root, strategy);
    errors.extend(validate_order_parameters(
        context,
        strategy.manage_stop,
        market_exit_order_constraints,
        &parameters.entry_order,
        &parameters.exit_order,
        &parameters.forced_exit_order,
    ));
    errors.extend(validate_parameter_bounds(
        context,
        &parameters,
        default_max_notional,
    ));
    errors.extend(validate_settlement_currency_derivable(
        context, root, strategy,
    ));
    errors
}

/// Fail closed when a settlement-booking strategy cannot derive settlement currency.
///
/// Uses the **same** [`settlement_currency_for_execution_account`] predicate the
/// runtime archetype applies — one owning match on (execution venue, account_id).
fn validate_settlement_currency_derivable(
    context: &str,
    root: &BoltV3RootConfig,
    strategy: &BoltV3StrategyConfig,
) -> Vec<String> {
    let execution_client_id = strategy.execution_client_id.as_str();
    let Some(execution_venue) = venue_for_client(root, execution_client_id) else {
        return vec![format!(
            "{context}: execution_client_id `{execution_client_id}` is not present in loaded clients; cannot derive settlement currency"
        )];
    };
    let Some(account_id) = execution_account_id(root, execution_client_id) else {
        return vec![format!(
            "{context}: execution client `{execution_client_id}` has no execution.account_id; cannot derive settlement currency for settlement booking"
        )];
    };
    if settlement_currency_for_execution_account(root, execution_venue, account_id).is_none() {
        return vec![format!(
            "{context}: settlement currency is not derivable for execution venue `{}` account `{account_id}`; configure risk.capital_pools with matching venue_id/account_id and collateral_currency (settlement booking requires it)",
            execution_venue.as_str()
        )];
    }
    Vec::new()
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
    SignalData {
        strategy_instance_id: String,
        message: String,
    },
    ResolutionData {
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
            Self::SignalData {
                strategy_instance_id,
                message,
            } => write!(
                f,
                "strategies.{strategy_instance_id} signal_data is invalid: {message}"
            ),
            Self::ResolutionData {
                strategy_instance_id,
                message,
            } => write!(
                f,
                "strategies.{strategy_instance_id} resolution_data is invalid: {message}"
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

pub fn prepare_runtime_strategy(
    context: StrategyRegistrationContext<'_>,
) -> Result<PreparedStrategyRegistration, BoltV3StrategyRegistrationError> {
    let raw = raw_taker_config(
        context.strategy,
        context.preparation_config(),
        context.prepared_client_routes(),
    )
    .map_err(|error| binding_error(&context, error))?;
    let build_context = assemble_strategy_build_context(&context)?;
    let registry = production_strategy_registry()
        .map_err(|error| binding_message(&context, error.to_string()))?;
    registry
        .prepare_strategy(BinaryOracleEdgeTakerBuilder::kind(), &raw, &build_context)
        .map_err(|error| binding_message(&context, error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bolt_v3_strategy_registration::settlement_currency_from_config_code;

    #[test]
    fn binary_oracle_archetype_maps_settlement_identity_from_capital_pool() {
        let loaded = crate::bolt_v3_config::load_bolt_v3_config(std::path::Path::new(
            "tests/fixtures/bolt_v3/root.toml",
        ))
        .expect("bolt-v3 fixture root should load");
        let strategy = loaded
            .strategies
            .iter()
            .find(|strategy| strategy.config.strategy_archetype.as_str() == KEY)
            .expect("fixture should include a binary oracle strategy");
        let execution_client_id = strategy.config.execution_client_id.as_str();
        let execution_venue = venue_for_client(&loaded.root, execution_client_id)
            .expect("fixture strategy execution client should exist");
        let account_id = execution_account_id(&loaded.root, execution_client_id)
            .expect("fixture execution client should bind an account id");
        let pool = loaded
            .root
            .risk
            .capital_pools
            .as_ref()
            .and_then(|pools| {
                pools.iter().find(|pool| {
                    pool.venue_id == execution_venue.as_str()
                        && pool.account_id.to_string() == account_id
                })
            })
            .expect("fixture capital pool should bind the strategy execution account");

        assert_eq!(pool.account_id.to_string(), account_id);
        assert_eq!(
            settlement_currency_for_execution_account(&loaded.root, execution_venue, account_id),
            settlement_currency_from_config_code(pool.collateral_currency.as_str())
        );
    }

    #[test]
    fn edge_taker_validation_fails_without_capital_pool_settlement_currency() {
        let mut loaded = crate::bolt_v3_config::load_bolt_v3_config(std::path::Path::new(
            "tests/fixtures/bolt_v3/root.toml",
        ))
        .expect("bolt-v3 fixture root should load");
        let strategy = loaded
            .strategies
            .iter()
            .find(|strategy| strategy.config.strategy_archetype.as_str() == KEY)
            .expect("fixture should include a binary oracle strategy")
            .config
            .clone();
        loaded.root.risk.capital_pools = None;
        let errors = validate_strategy("strategies.test", &loaded.root, &strategy, None);
        assert!(
            errors.iter().any(|error| {
                error.contains("settlement currency is not derivable")
                    && error.contains("risk.capital_pools")
            }),
            "deployed profile shape without a matching capital pool must fail load-time validation; errors={errors:?}"
        );
    }

    #[test]
    fn edge_taker_validation_passes_with_capital_pool_settlement_currency() {
        let loaded = crate::bolt_v3_config::load_bolt_v3_config(std::path::Path::new(
            "tests/fixtures/bolt_v3/root.toml",
        ))
        .expect("bolt-v3 fixture root should load");
        let strategy = loaded
            .strategies
            .iter()
            .find(|strategy| strategy.config.strategy_archetype.as_str() == KEY)
            .expect("fixture should include a binary oracle strategy");
        let errors = validate_strategy("strategies.test", &loaded.root, &strategy.config, None);
        assert!(
            errors
                .iter()
                .all(|error| !error.contains("settlement currency is not derivable")),
            "fixture with matching capital pool must pass settlement-currency validation; errors={errors:?}"
        );
    }

    #[test]
    fn production_root_derives_pusd_settlement_currency_for_polymarket_execution_account() {
        let loaded =
            crate::bolt_v3_config::load_bolt_v3_config(std::path::Path::new("config/root.toml"))
                .expect("production root.toml should load");
        let execution_venue = venue_for_client(&loaded.root, "polymarket_main")
            .expect("production root must declare polymarket_main");
        let account_id = execution_account_id(&loaded.root, "polymarket_main")
            .expect("production polymarket_main must bind execution.account_id");
        let currency =
            settlement_currency_for_execution_account(&loaded.root, execution_venue, account_id);
        assert_eq!(
            currency,
            settlement_currency_from_config_code("pUSD"),
            "production root capital pool must derive pUSD for POLYMARKET / POLYMARKET-001"
        );
    }
}

pub fn raw_taker_config(
    strategy: &LoadedStrategy,
    preparation_config: &StrategyPreparationConfig,
    client_routes: &PreparedStrategyClientRoutes,
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
    let strategy_instance_id = strategy.config.strategy_instance_id.as_str();
    let realized_volatility_surface_id = strategy
        .config
        .realized_volatility_surface_id
        .as_deref()
        .ok_or_else(|| BinaryOracleEdgeTakerRuntimeConfigError::Parameters {
            strategy_instance_id: strategy.config.strategy_instance_id.clone(),
            message: "config.realized_volatility_surface_id is required".to_string(),
        })?;
    let realized_volatility_max_source_age_ms = preparation_config
        .realized_volatility_max_source_age_ms(realized_volatility_surface_id)
        .ok_or_else(|| BinaryOracleEdgeTakerRuntimeConfigError::Parameters {
            strategy_instance_id: strategy.config.strategy_instance_id.clone(),
            message: format!(
                "configured surface `{realized_volatility_surface_id}` is not present in realized_volatility_surfaces"
            ),
        })?;
    if realized_volatility_max_source_age_ms == 0 {
        return Err(BinaryOracleEdgeTakerRuntimeConfigError::Parameters {
            strategy_instance_id: strategy.config.strategy_instance_id.clone(),
            message: format!(
                "realized_volatility_surfaces.{realized_volatility_surface_id}.policy.max_source_age_ms must be positive"
            ),
        });
    }
    let reference_current_price = strategy
        .config
        .reference_current_price
        .as_ref()
        .ok_or_else(|| BinaryOracleEdgeTakerRuntimeConfigError::Parameters {
            strategy_instance_id: strategy.config.strategy_instance_id.clone(),
            message: "reference_current_price is required".to_string(),
        })?;
    let signal_data = configured_signal_data(strategy)?;
    validate_configured_decision_reference(strategy_instance_id, &strategy.config.target)?;
    client_routes
        .venue(&signal_data.data_client_id)
        .ok_or_else(|| BinaryOracleEdgeTakerRuntimeConfigError::SignalData {
            strategy_instance_id: strategy.config.strategy_instance_id.clone(),
            message: format!(
                "signal_data data_client_id `{}` is not present in loaded clients",
                signal_data.data_client_id
            ),
        })?;
    let resolution_data = configured_resolution_data(strategy);
    if let Some(resolution_data) = resolution_data {
        let resolution_venue = client_routes
            .venue(&resolution_data.data_client_id)
            .ok_or_else(|| BinaryOracleEdgeTakerRuntimeConfigError::ResolutionData {
                strategy_instance_id: strategy.config.strategy_instance_id.clone(),
                message: format!(
                    "resolution_data data_client_id `{}` is not present in loaded clients",
                    resolution_data.data_client_id
                ),
            })?;
        validate_resolution_data_binding(
            strategy,
            resolution_data,
            preparation_config,
            resolution_venue,
            &target.underlying_asset,
        )?;
    }

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
    let price_to_beat_source =
        price_to_beat_source_from_target(strategy_instance_id, &strategy.config.target)?;
    let resolution_provider_id =
        resolution_gate_provider_id_from_target(strategy_instance_id, &strategy.config.target)?;
    let forced_flat_stale_reference_ms = preparation_config
        .gate_provider_max_age_ms(&resolution_provider_id)
        .filter(|value| *value != 0)
        .ok_or_else(|| BinaryOracleEdgeTakerRuntimeConfigError::Target {
            strategy_instance_id: strategy_instance_id.to_string(),
            message: format!(
                "gate_providers.{resolution_provider_id}.freshness.max_age_ms is required for forced_flat_stale_reference_ms"
            ),
        })?;

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
    insert_optional_string(
        &mut table,
        "static_condition_id",
        target.static_condition_id,
    );
    insert_optional_string(&mut table, "static_yes_outcome", target.static_yes_outcome);
    insert_optional_string(&mut table, "static_no_outcome", target.static_no_outcome);
    insert_optional_string(
        &mut table,
        "static_fair_probability_source",
        target.static_fair_probability_source,
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
    insert_string(&mut table, "price_to_beat_source", price_to_beat_source);
    insert_reference_current_price_config(
        &mut table,
        strategy_instance_id,
        reference_current_price,
    )?;
    insert_string(
        &mut table,
        "signal_venue",
        signal_data.data_client_id.to_string(),
    );
    insert_string(
        &mut table,
        "signal_instrument_id",
        signal_data.instrument_id.to_string(),
    );
    insert_string(
        &mut table,
        "realized_volatility_surface_id",
        realized_volatility_surface_id.to_string(),
    );
    insert_u64(
        &mut table,
        strategy_instance_id,
        "realized_volatility_max_source_age_ms",
        realized_volatility_max_source_age_ms,
    )?;
    if let Some(resolution_data) = resolution_data {
        insert_string(
            &mut table,
            "resolution_client_id",
            resolution_data.data_client_id.to_string(),
        );
        insert_string(
            &mut table,
            "resolution_instrument_id",
            resolution_data.instrument_id.to_string(),
        );
    }
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
    insert_order_config(
        &mut table,
        strategy_instance_id,
        "forced_exit_order",
        &parameters.forced_exit_order,
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
    insert_u64(
        &mut table,
        strategy_instance_id,
        "vwap_depth_limit_bps",
        parameters.runtime.vwap_depth_limit_bps,
    )?;
    insert_u64(
        &mut table,
        strategy_instance_id,
        "slippage_buffer_bps",
        parameters.runtime.slippage_buffer_bps,
    )?;
    insert_float(&mut table, "risk_lambda", parameters.runtime.risk_lambda);
    insert_u64(
        &mut table,
        strategy_instance_id,
        "sizing_ev_reference_bps",
        parameters.runtime.sizing_ev_reference_bps,
    )?;
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
        "trade_flow_window_secs",
        parameters.runtime.trade_flow_window_secs,
    )?;
    insert_u64(
        &mut table,
        strategy_instance_id,
        "trade_flow_max_samples",
        parameters.runtime.trade_flow_max_samples,
    )?;
    insert_float(
        &mut table,
        "spike_guard_return_threshold",
        parameters.runtime.spike_guard_return_threshold,
    );
    insert_u64(
        &mut table,
        strategy_instance_id,
        "spike_guard_cooldown_secs",
        parameters.runtime.spike_guard_cooldown_secs,
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
        forced_flat_stale_reference_ms,
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

fn price_to_beat_source_from_target(
    strategy_instance_id: &str,
    target: &toml::Value,
) -> Result<String, BinaryOracleEdgeTakerRuntimeConfigError> {
    let subscription = resolution_subscription_table(strategy_instance_id, target)?;
    let mapping = first_resolution_market_mapping(strategy_instance_id, subscription)?;
    let resolution_kind =
        required_resolution_mapping_string(strategy_instance_id, mapping, "resolution_kind")?;
    let resolution_identity =
        required_resolution_mapping_string(strategy_instance_id, mapping, "resolution_identity")?;
    Ok(format!("{}.{}", resolution_kind, resolution_identity))
}

fn resolution_gate_provider_id_from_target(
    strategy_instance_id: &str,
    target: &toml::Value,
) -> Result<String, BinaryOracleEdgeTakerRuntimeConfigError> {
    let subscription = resolution_subscription_table(strategy_instance_id, target)?;
    let mapping = first_resolution_market_mapping(strategy_instance_id, subscription)?;
    gate_provider_id_from_subscription(
        strategy_instance_id,
        RESOLUTION_GATE_ROLE,
        subscription,
        mapping,
    )
}

fn validate_configured_decision_reference(
    strategy_instance_id: &str,
    target: &toml::Value,
) -> Result<(), BinaryOracleEdgeTakerRuntimeConfigError> {
    let Some(subscription) = target
        .as_table()
        .and_then(|target| target.get("gate_subscriptions"))
        .and_then(toml::Value::as_table)
        .and_then(|subscriptions| subscriptions.get(DECISION_REFERENCE_GATE_ROLE))
        .and_then(toml::Value::as_table)
    else {
        return Ok(());
    };
    let mapping = first_gate_market_mapping(
        strategy_instance_id,
        DECISION_REFERENCE_GATE_ROLE,
        subscription,
    )?;
    let _provider_id = gate_provider_id_from_subscription(
        strategy_instance_id,
        DECISION_REFERENCE_GATE_ROLE,
        subscription,
        mapping,
    )?;
    let resolution_identity =
        required_resolution_mapping_string(strategy_instance_id, mapping, "resolution_identity")?
            .to_string();
    // decision_reference is a source-owned gate identity, not an NT quote
    // instrument. Reject parseable InstrumentIds so source-gate config cannot be
    // mistaken for venue quote subscription config.
    if resolution_identity.parse::<InstrumentId>().is_ok() {
        return Err(BinaryOracleEdgeTakerRuntimeConfigError::Target {
            strategy_instance_id: strategy_instance_id.to_string(),
            message: format!(
                "decision_reference resolution_identity `{resolution_identity}` must be a logical gate identity, not a value that parses as an NT instrument id (which would make the source-owned reference path subscribe to venue quotes)"
            ),
        });
    }
    Ok(())
}

fn gate_provider_id_from_subscription(
    strategy_instance_id: &str,
    gate_role: &str,
    subscription: &toml::map::Map<String, toml::Value>,
    mapping: &toml::map::Map<String, toml::Value>,
) -> Result<String, BinaryOracleEdgeTakerRuntimeConfigError> {
    mapping
        .get("provider_id")
        .and_then(toml::Value::as_str)
        .or_else(|| {
            subscription
                .get("provider_preference")
                .and_then(toml::Value::as_array)
                .and_then(|provider_ids| provider_ids.first())
                .and_then(toml::Value::as_str)
        })
        .or_else(|| {
            let provider_ids = subscription
                .get("allowed_provider_ids")
                .and_then(toml::Value::as_array)?;
            (provider_ids.len() == 1)
                .then(|| provider_ids[0].as_str())
                .flatten()
        })
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| BinaryOracleEdgeTakerRuntimeConfigError::Target {
            strategy_instance_id: strategy_instance_id.to_string(),
            message: format!("{gate_role} gate provider_id is required for runtime bridge"),
        })
}

fn first_resolution_market_mapping<'a>(
    strategy_instance_id: &str,
    subscription: &'a toml::map::Map<String, toml::Value>,
) -> Result<&'a toml::map::Map<String, toml::Value>, BinaryOracleEdgeTakerRuntimeConfigError> {
    first_gate_market_mapping(strategy_instance_id, RESOLUTION_GATE_ROLE, subscription)
}

fn first_gate_market_mapping<'a>(
    strategy_instance_id: &str,
    gate_role: &str,
    subscription: &'a toml::map::Map<String, toml::Value>,
) -> Result<&'a toml::map::Map<String, toml::Value>, BinaryOracleEdgeTakerRuntimeConfigError> {
    subscription
        .get("market_mappings")
        .and_then(toml::Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
        .iter()
        .filter_map(toml::Value::as_table)
        .next()
        .ok_or_else(|| BinaryOracleEdgeTakerRuntimeConfigError::Target {
            strategy_instance_id: strategy_instance_id.to_string(),
            message: format!("target gate market_mappings must include a {gate_role} mapping"),
        })
}

fn required_resolution_mapping_string<'a>(
    strategy_instance_id: &str,
    mapping: &'a toml::map::Map<String, toml::Value>,
    field: &'static str,
) -> Result<&'a str, BinaryOracleEdgeTakerRuntimeConfigError> {
    mapping
        .get(field)
        .and_then(toml::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| BinaryOracleEdgeTakerRuntimeConfigError::Target {
            strategy_instance_id: strategy_instance_id.to_string(),
            message: format!("target gate {field} is required for price_to_beat_source"),
        })
}

fn resolution_subscription_table<'a>(
    strategy_instance_id: &str,
    target: &'a toml::Value,
) -> Result<&'a toml::map::Map<String, toml::Value>, BinaryOracleEdgeTakerRuntimeConfigError> {
    target
        .as_table()
        .and_then(|target| target.get("gate_subscriptions"))
        .and_then(toml::Value::as_table)
        .and_then(|subscriptions| subscriptions.get(RESOLUTION_GATE_ROLE))
        .and_then(toml::Value::as_table)
        .ok_or_else(|| BinaryOracleEdgeTakerRuntimeConfigError::Target {
            strategy_instance_id: strategy_instance_id.to_string(),
            message: format!(
                "target.gate_subscriptions.{RESOLUTION_GATE_ROLE} is required for runtime bridge"
            ),
        })
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

fn usize_to_u64(
    strategy_instance_id: &str,
    field: &'static str,
    value: usize,
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

fn insert_optional_string(
    table: &mut Map<String, Value>,
    key: &'static str,
    value: Option<String>,
) {
    if let Some(value) = value {
        insert_string(table, key, value);
    }
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

fn insert_reference_current_price_config(
    table: &mut Map<String, Value>,
    strategy_instance_id: &str,
    reference_current_price: &crate::bolt_v3_config::ReferencePriceBlock,
) -> Result<(), BinaryOracleEdgeTakerRuntimeConfigError> {
    let mut reference_table = Map::new();
    insert_string(
        &mut reference_table,
        "asset",
        reference_current_price.asset.clone(),
    );
    insert_string_array(
        &mut reference_table,
        "sources",
        &reference_current_price.source_order,
    );
    insert_u64(
        &mut reference_table,
        strategy_instance_id,
        "min_valid_sources",
        usize_to_u64(
            strategy_instance_id,
            "reference_current_price.min_valid_sources",
            reference_current_price.min_valid_sources,
        )?,
    )?;
    insert_string(
        &mut reference_table,
        "selection_policy",
        reference_price_selection_policy_value(reference_current_price.selection_policy),
    );
    insert_u64(
        &mut reference_table,
        strategy_instance_id,
        "max_source_age_ms",
        reference_current_price.max_source_age_ms,
    )?;
    insert_u64(
        &mut reference_table,
        strategy_instance_id,
        "max_source_drift_bps",
        u64::from(reference_current_price.max_source_drift_bps),
    )?;
    insert_string(
        &mut reference_table,
        "drift_policy",
        reference_price_drift_policy_value(reference_current_price.drift_policy),
    );
    insert_string(
        &mut reference_table,
        "stale_policy",
        reference_price_stale_policy_value(reference_current_price.stale_policy),
    );

    let mut source_tables = Map::new();
    for (source_id, source) in &reference_current_price.sources {
        let mut source_table = Map::new();
        insert_string(
            &mut source_table,
            "provider",
            reference_price_provider_value(&source.provider),
        );
        insert_bool(&mut source_table, "enabled", source.enabled);
        insert_bool(&mut source_table, "required", source.required);
        insert_string(&mut source_table, "client_id", source.client_id.to_string());
        if let Some(instrument_id) = &source.instrument_id {
            insert_string(&mut source_table, "instrument_id", instrument_id.clone());
        }
        if let Some(symbol) = &source.symbol {
            insert_string(&mut source_table, "symbol", symbol.clone());
        }
        source_tables.insert(source_id.clone(), Value::Table(source_table));
    }
    reference_table.insert("source".to_string(), Value::Table(source_tables));
    table.insert(
        "reference_current_price".to_string(),
        Value::Table(reference_table),
    );
    Ok(())
}

fn reference_price_provider_value(
    provider: &crate::bolt_v3_config::ReferencePriceProvider,
) -> String {
    provider.as_str().to_string()
}

fn reference_price_selection_policy_value(
    policy: crate::bolt_v3_config::ReferencePriceSelectionPolicy,
) -> String {
    match policy {
        crate::bolt_v3_config::ReferencePriceSelectionPolicy::FirstValidPerInterval => {
            "first_valid_per_interval"
        }
    }
    .to_string()
}

fn reference_price_drift_policy_value(
    policy: crate::bolt_v3_config::ReferencePriceDriftPolicy,
) -> String {
    match policy {
        crate::bolt_v3_config::ReferencePriceDriftPolicy::Observe => "observe",
        crate::bolt_v3_config::ReferencePriceDriftPolicy::Block => "block",
    }
    .to_string()
}

fn reference_price_stale_policy_value(
    policy: crate::bolt_v3_config::ReferencePriceStalePolicy,
) -> String {
    match policy {
        crate::bolt_v3_config::ReferencePriceStalePolicy::Block => "block",
    }
    .to_string()
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
    if let Some(activation_price) = order.activation_price {
        let activation_price = activation_price.to_f64().ok_or_else(|| {
            BinaryOracleEdgeTakerRuntimeConfigError::Numeric {
                strategy_instance_id: strategy_instance_id.to_string(),
                field: "activation_price",
                value: activation_price.to_string(),
            }
        })?;
        insert_float(&mut order_table, "activation_price", activation_price);
    }
    if let Some(trigger_type) = order.trigger_type {
        insert_string(
            &mut order_table,
            "trigger_type",
            enum_variant_lowercase(trigger_type),
        );
    }
    if let Some(trigger_instrument_id) = order.trigger_instrument_id {
        insert_string(
            &mut order_table,
            "trigger_instrument_id",
            trigger_instrument_id.to_string(),
        );
    }
    if let Some(trailing_offset) = order.trailing_offset {
        let trailing_offset = trailing_offset.to_f64().ok_or_else(|| {
            BinaryOracleEdgeTakerRuntimeConfigError::Numeric {
                strategy_instance_id: strategy_instance_id.to_string(),
                field: "trailing_offset",
                value: trailing_offset.to_string(),
            }
        })?;
        insert_float(&mut order_table, "trailing_offset", trailing_offset);
    }
    if let Some(trailing_offset_type) = order.trailing_offset_type {
        insert_string(
            &mut order_table,
            "trailing_offset_type",
            enum_variant_lowercase(trailing_offset_type),
        );
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

fn configured_signal_data(
    strategy: &LoadedStrategy,
) -> Result<&crate::bolt_v3_config::DataInstrumentBlock, BinaryOracleEdgeTakerRuntimeConfigError> {
    let mut entries = strategy.config.signal_data.iter();
    match (entries.next(), entries.next()) {
        (Some((_role, block)), None) => Ok(block),
        (None, _) => Err(BinaryOracleEdgeTakerRuntimeConfigError::SignalData {
            strategy_instance_id: strategy.config.strategy_instance_id.clone(),
            message: "requires exactly one [signal_data.<role>] block".to_string(),
        }),
        (Some(_), Some(_)) => Err(BinaryOracleEdgeTakerRuntimeConfigError::SignalData {
            strategy_instance_id: strategy.config.strategy_instance_id.clone(),
            message: format!(
                "allows at most one [signal_data.<role>] block; got roles [{}]",
                signal_data_role_names(&strategy.config)
            ),
        }),
    }
}

fn configured_resolution_data(
    strategy: &LoadedStrategy,
) -> Option<&crate::bolt_v3_config::DataInstrumentBlock> {
    strategy.config.resolution_data.as_ref()
}

/// Fail-closed load-time validation of a `resolution_data` strike binding.
///
/// The resolution (strike) feed is the Chainlink Data Streams source, so a
/// `resolution_data` block must (a) point at a client whose venue is the
/// Chainlink venue, (b) name an instrument whose asset prefix matches the
/// target's `underlying_asset`, and (c) name an instrument that has a
/// root-owned `chainlink_data_streams.feed_bindings` entry. Any mismatch can
/// only ever fail at subscribe time (no report, or the wrong asset's strike),
/// so it is rejected here at config load.
fn validate_resolution_data_binding(
    strategy: &LoadedStrategy,
    resolution_data: &crate::bolt_v3_config::DataInstrumentBlock,
    preparation_config: &StrategyPreparationConfig,
    resolution_venue: Venue,
    underlying_asset: &str,
) -> Result<(), BinaryOracleEdgeTakerRuntimeConfigError> {
    let reject = |message: String| BinaryOracleEdgeTakerRuntimeConfigError::ResolutionData {
        strategy_instance_id: strategy.config.strategy_instance_id.clone(),
        message,
    };

    // (a) the preflight-resolved venue must be the resolution-oracle strike provider.
    if resolution_venue.as_str() != crate::bolt_v3_providers::RESOLUTION_ORACLE_VENUE_KEY {
        return Err(reject(format!(
            "data_client_id `{}` has venue `{}`, but the strike feed must be served by a `{}` client",
            resolution_data.data_client_id,
            resolution_venue,
            crate::bolt_v3_providers::RESOLUTION_ORACLE_VENUE_KEY
        )));
    }

    // (b) instrument asset prefix must match the target's underlying_asset.
    let symbol = resolution_data.instrument_id.symbol.as_str();
    let instrument_asset = symbol.split_once('-').map_or(symbol, |(asset, _)| asset);
    if instrument_asset != underlying_asset {
        return Err(reject(format!(
            "instrument_id `{}` resolves to asset `{instrument_asset}`, which does not match the target underlying_asset `{underlying_asset}`",
            resolution_data.instrument_id
        )));
    }

    // (c) instrument must have a root-owned feed_binding.
    let instrument_id = resolution_data.instrument_id.to_string();
    if !preparation_config.has_chainlink_feed_binding(&instrument_id) {
        return Err(reject(format!(
            "instrument_id `{}` has no matching feed_binding in root chainlink_data_streams.feed_bindings for client `{}`",
            resolution_data.instrument_id, resolution_data.data_client_id
        )));
    }

    Ok(())
}

fn validate_resolution_retry_interval_covers_http_timeout(
    context: &str,
    root: &BoltV3RootConfig,
    strategy: &BoltV3StrategyConfig,
) -> Vec<String> {
    let Some(resolution_data) = strategy.resolution_data.as_ref() else {
        return Vec::new();
    };
    let Ok(target) =
        crate::bolt_v3_market_families::target_runtime_fields_from_target(&strategy.target)
    else {
        return Vec::new();
    };
    let client_key = resolution_data.data_client_id.to_string();
    match resolution_oracle_client_http_timeout_secs(root, client_key.as_str()) {
        Ok(Some(http_timeout_secs)) if target.retry_interval_seconds <= http_timeout_secs => {
            vec![format!(
                "{context}: target.retry_interval_secs `{}` must be greater than clients.{client_key}.data.http_timeout_secs `{http_timeout_secs}` for resolution_data settlement-close retries; otherwise same-boundary in-flight fetch dedupe can consume market_exit_max_attempts before the first HTTP request times out",
                target.retry_interval_seconds,
            )]
        }
        Ok(_) => Vec::new(),
        Err(message) => vec![format!(
            "{context}: resolution_data data_client_id `{client_key}` could not validate resolution-oracle http_timeout_secs: {message}"
        )],
    }
}

fn signal_data_role_names(strategy: &BoltV3StrategyConfig) -> String {
    strategy
        .signal_data
        .keys()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(", ")
}

fn validate_required_market_data(context: &str, strategy: &BoltV3StrategyConfig) -> Vec<String> {
    let mut errors = Vec::new();
    if strategy.signal_data.len() > 1 {
        errors.push(format!(
            "{context}: strategy_archetype `binary_oracle_edge_taker` allows at most one [signal_data.<role>] block"
        ));
    }
    if strategy.signal_data.is_empty() {
        errors.push(format!(
            "{context}: strategy_archetype `binary_oracle_edge_taker` requires exactly one [signal_data.<role>] block"
        ));
    }
    if strategy.reference_current_price.is_none() {
        errors.push(format!(
            "{context}: strategy_archetype `binary_oracle_edge_taker` requires [reference_current_price] block"
        ));
    }
    errors
}

fn validate_reference_current_price_forced_flat_grace(
    context: &str,
    root: &BoltV3RootConfig,
    strategy: &BoltV3StrategyConfig,
) -> Vec<String> {
    let Some(reference_current_price) = &strategy.reference_current_price else {
        return Vec::new();
    };
    let Ok(target) =
        crate::bolt_v3_market_families::target_runtime_fields_from_target(&strategy.target)
    else {
        return Vec::new();
    };
    let Ok(provider_id) = resolution_gate_provider_id_from_target(
        strategy.strategy_instance_id.as_str(),
        &strategy.target,
    ) else {
        return Vec::new();
    };
    let Some(forced_flat_stale_reference_ms) = root
        .gate_providers
        .as_ref()
        .and_then(|providers| providers.get(provider_id.as_str()))
        .and_then(|provider| provider.freshness.as_ref())
        .and_then(|freshness| freshness.max_age_ms)
        .filter(|value| *value != 0)
    else {
        return Vec::new();
    };

    let retry_interval_ms = target
        .retry_interval_seconds
        .saturating_mul(MILLIS_PER_SECOND_U64);
    let required_minimum = reference_current_price
        .max_source_age_ms
        .saturating_add(retry_interval_ms);
    if forced_flat_stale_reference_ms <= required_minimum {
        return vec![format!(
            "{context}: forced_flat_stale_reference_ms `{forced_flat_stale_reference_ms}` from gate_providers.{provider_id}.freshness.max_age_ms must be greater than reference_current_price.max_source_age_ms `{}` plus target retry_interval `{retry_interval_ms}` ms",
            reference_current_price.max_source_age_ms,
        )];
    }

    Vec::new()
}

fn validate_order_parameters(
    context: &str,
    manage_stop: bool,
    market_exit_order_constraints: Option<ProviderMarketExitOrderConstraints>,
    entry: &OrderParams,
    exit: &OrderParams,
    forced_exit: &OrderParams,
) -> Vec<String> {
    let mut errors = Vec::new();
    errors.extend(check_strategy_position_contract(context, entry, exit));
    errors.extend(check_entry_order_combination(context, entry));
    errors.extend(check_exit_order_combination(
        context,
        market_exit_order_constraints,
        exit,
    ));
    errors.extend(check_forced_exit_order_combination(
        context,
        manage_stop,
        market_exit_order_constraints,
        exit,
        forced_exit,
    ));
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
    let position_max_decimal = match crate::bolt_v3_validate::parse_decimal_string(
        &parameters.maximum_position_notional,
    ) {
        Ok(value) => Some(value),
        Err(reason) => {
            errors.push(format!(
                "{context}: parameters.maximum_position_notional is not a valid decimal string ({reason}): `{}`",
                parameters.maximum_position_notional
            ));
            None
        }
    };
    // Both sizing caps must be strictly positive at load. A zero/negative cap is
    // nonsensical config: the runtime sizing path clamps it to zero and the
    // strategy silently never submits (fail-soft), so the misconfiguration must
    // fail closed here instead of at the first dead trading session.
    if let Some(order_target) = order_target_decimal.as_ref()
        && *order_target <= Decimal::ZERO
    {
        errors.push(format!(
            "{context}: parameters.order_notional_target ({order_target}) must be a positive decimal"
        ));
    }
    if let Some(position_max) = position_max_decimal.as_ref()
        && *position_max <= Decimal::ZERO
    {
        errors.push(format!(
            "{context}: parameters.maximum_position_notional ({position_max}) must be a positive decimal"
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
    // The per-order target cannot exceed the maximum total position notional; such a
    // target is unsatisfiable (the position cap would always bind first).
    if let (Some(order_target), Some(position_max)) =
        (order_target_decimal.as_ref(), position_max_decimal.as_ref())
        && order_target > position_max
    {
        errors.push(format!(
            "{context}: parameters.order_notional_target ({order_target}) must be <= parameters.maximum_position_notional ({position_max})"
        ));
    }
    // A negative edge threshold admits negative-edge (guaranteed-loss) entries: the entry
    // edge gate `expected_edge > threshold * theta` is satisfied for any edge — including
    // losing ones — when the threshold is negative. Fail closed at load (A-EDGE).
    if parameters.edge_threshold_basis_points < 0 {
        errors.push(format!(
            "{context}: parameters.edge_threshold_basis_points ({}) must be >= 0 (a negative edge threshold admits negative-edge / guaranteed-loss entries)",
            parameters.edge_threshold_basis_points
        ));
    }
    // The EV sizing reference anchors the dollar scale: worst-case EV at
    // 2*risk_lambda*reference saturates sizing at order_notional_target. A zero
    // reference makes that scale division undefined; the runtime sizing path
    // fails closed to a zero size and the strategy silently never submits, so
    // the misconfiguration must fail closed at load instead.
    if parameters.runtime.sizing_ev_reference_bps == 0 {
        errors.push(format!(
            "{context}: parameters.runtime.sizing_ev_reference_bps must be > 0 (sizing saturates at order_notional_target when worst-case EV reaches 2 * risk_lambda * sizing_ev_reference_bps)"
        ));
    }
    if (parameters.runtime.sizing_ev_reference_bps as f64) > crate::bolt_v3_numeric::BPS_DENOMINATOR
    {
        errors.push(format!(
            "{context}: parameters.runtime.sizing_ev_reference_bps must be at most {} bps",
            crate::bolt_v3_numeric::BPS_DENOMINATOR
        ));
    }
    // TOML floats legally admit negative, nan, and inf. Each loads through
    // serde but makes the runtime sizing path fail soft to a zero size (a
    // silently dead strategy), so each must fail closed at load. Zero stays
    // valid: it is the deliberate caution-off escape hatch (size = cap).
    if !crate::bolt_v3_numeric::is_non_negative_finite(parameters.runtime.risk_lambda) {
        errors.push(format!(
            "{context}: parameters.runtime.risk_lambda ({}) must be finite and >= 0 (zero disables risk scaling; negative/nan/inf silently size every order to zero)",
            parameters.runtime.risk_lambda
        ));
    }
    errors
}

fn check_entry_order_combination(context: &str, entry: &OrderParams) -> Vec<String> {
    let mut errors = check_enabled_order_template(context, stringify!(entry_order), entry);
    if !executable_entry_order_shape_supported(entry) {
        errors.push(archetype_validation_error(
            context,
            BINARY_ORACLE_ENTRY_ORDER_UNSUPPORTED_SHAPE_CODE,
            "parameters.entry_order unsupported executable entry shape: must be buy/long market FOK quote-quantity without post-only, trigger, or trailing fields",
        ));
    }
    if entry.is_reduce_only {
        errors.push(archetype_validation_error(
            context,
            BINARY_ORACLE_ENTRY_ORDER_REDUCE_ONLY_CODE,
            "parameters.entry_order.is_reduce_only must be false because `binary_oracle_edge_taker` entry orders open the managed position",
        ));
    }
    errors
}

fn archetype_validation_error(context: &str, code: &str, message: &str) -> String {
    format!("{context}: error_code={code} {message}")
}

fn executable_entry_order_shape_supported(entry: &OrderParams) -> bool {
    // Fields with dedicated entry diagnostics stay out of this broad shape predicate
    // so operators see one specific error for those cases.
    entry.side == OrderSide::Buy
        && entry.position_side == PositionSide::Long
        && entry.order_type == OrderType::Market
        && entry.time_in_force == TimeInForce::Fok
        && entry.is_quote_quantity
        && !entry.is_post_only
        && entry.trigger_price.is_none()
        && entry.activation_price.is_none()
        && entry.trigger_type.is_none()
        && entry.trigger_instrument_id.is_none()
        && entry.trailing_offset.is_none()
        && entry.trailing_offset_type.is_none()
}

fn check_exit_order_combination(
    context: &str,
    market_exit_order_constraints: Option<ProviderMarketExitOrderConstraints>,
    exit: &OrderParams,
) -> Vec<String> {
    let mut errors = check_enabled_order_template(context, stringify!(exit_order), exit);
    errors.extend(check_provider_market_exit_shape(
        context,
        stringify!(exit_order),
        exit,
        market_exit_order_constraints,
    ));
    if exit.is_quote_quantity {
        errors.push(format!(
            "{context}: parameters.exit_order.is_quote_quantity=true is not supported because `binary_oracle_edge_taker` exits are sized from base position quantity"
        ));
    }
    errors
}

fn check_forced_exit_order_combination(
    context: &str,
    manage_stop: bool,
    market_exit_order_constraints: Option<ProviderMarketExitOrderConstraints>,
    exit: &OrderParams,
    forced_exit: &OrderParams,
) -> Vec<String> {
    let mut errors =
        check_enabled_order_template(context, stringify!(forced_exit_order), forced_exit);
    errors.extend(check_provider_market_exit_shape(
        context,
        stringify!(forced_exit_order),
        forced_exit,
        market_exit_order_constraints,
    ));
    if forced_exit.is_quote_quantity {
        errors.push(format!(
            "{context}: parameters.forced_exit_order.is_quote_quantity=true is not supported because `binary_oracle_edge_taker` forced exits are sized from base position quantity"
        ));
    }
    if forced_exit.side != exit.side || forced_exit.position_side != exit.position_side {
        errors.push(format!(
            "{context}: parameters.forced_exit_order side and position_side must match parameters.exit_order for `binary_oracle_edge_taker`"
        ));
    }
    if manage_stop && forced_exit.order_type != OrderType::Market {
        errors.push(format!(
            "{context}: manage_stop=true uses NautilusTrader Strategy::close_all_positions market orders, so parameters.forced_exit_order.order_type must be `market`; set manage_stop=false to use a non-market forced_exit_order through the strategy forced-flat path"
        ));
    }
    errors
}

fn strategy_execution_client_market_exit_order_constraints(
    root: &BoltV3RootConfig,
    strategy: &BoltV3StrategyConfig,
) -> Option<ProviderMarketExitOrderConstraints> {
    let execution_venue = venue_for_client(root, strategy.execution_client_id.as_str())?;
    let binding = binding_for_provider_key(execution_venue.as_str())?;
    Some(binding.market_exit_order_constraints)
}

fn check_provider_market_exit_shape(
    context: &str,
    field: &str,
    order: &OrderParams,
    constraints: Option<ProviderMarketExitOrderConstraints>,
) -> Vec<String> {
    let Some(constraints) = constraints else {
        return Vec::new();
    };

    let mut errors = Vec::new();
    if order.is_reduce_only && !constraints.reduce_only_supported {
        errors.push(format!(
            "{context}: parameters.{field}.is_reduce_only must be false because the configured execution provider rejects reduce-only exits before submit"
        ));
    }
    if order.order_type != OrderType::Market {
        return errors;
    }
    if let Some(allowed_time_in_forces) = constraints.allowed_market_time_in_forces
        && !allowed_time_in_forces.contains(&order.time_in_force)
    {
        let configured = time_in_force_config_label(order.time_in_force);
        let allowed = allowed_time_in_forces_config_label(allowed_time_in_forces);
        errors.push(format!(
            "{context}: parameters.{field} order_type=market has time_in_force={configured}; must use time_in_force={allowed} because the configured execution provider rejects unsupported market time-in-force values before submit"
        ));
    }
    errors
}

fn time_in_force_config_label(time_in_force: TimeInForce) -> String {
    time_in_force.to_string().to_ascii_lowercase()
}

fn allowed_time_in_forces_config_label(time_in_forces: &[TimeInForce]) -> String {
    time_in_forces
        .iter()
        .copied()
        .map(time_in_force_config_label)
        .collect::<Vec<_>>()
        .join(" or ")
}

fn check_enabled_order_template(context: &str, field: &str, order: &OrderParams) -> Vec<String> {
    let field_path = format!("parameters.{field}");
    check_nt_order_template_config(context, &field_path, &order.nt_order_template_config())
}

fn check_strategy_position_contract(
    context: &str,
    entry: &OrderParams,
    exit: &OrderParams,
) -> Vec<String> {
    if entry.position_side == PositionSide::Short || exit.position_side == PositionSide::Short {
        return vec![format!(
            "{context}: short-side position contracts require strategy-owned short economics and are not enabled for `binary_oracle_edge_taker`"
        )];
    }
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
             long requires entry side=buy, exit side=sell, position_side=long"
        )]
    }
}
