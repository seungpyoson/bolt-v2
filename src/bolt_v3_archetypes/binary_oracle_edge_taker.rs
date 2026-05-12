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

use rust_decimal::Decimal;
use serde::Deserialize;

use crate::{
    bolt_v3_archetypes::{ArchetypeValidationBinding, ReferenceCapabilityRequirement},
    bolt_v3_config::BoltV3StrategyConfig,
    bolt_v3_providers::ReferenceCapability,
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

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ParametersBlock {
    pub edge_threshold_basis_points: i64,
    pub order_notional_target: String,
    pub maximum_position_notional: String,
    pub entry_order: OrderParams,
    pub exit_order: OrderParams,
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
