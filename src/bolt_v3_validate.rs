//! Startup-shaping validation for bolt-v3 root and strategy configs.
//!
//! Schema rules: docs/bolt-v3/2026-04-25-bolt-v3-schema.md Section 8.
//!
//! This module owns common strategy-envelope validation (schema
//! version, uniqueness of instance / order-id-tag, client / execution
//! lookup, per-role reference-data structural validation, reference quote
//! probe source disambiguation), root-block validation, and root risk
//! decimal syntax only. Market-family-shaped
//! target rules
//! (rotating-market kind, family discriminator, cadence policy,
//! underlying-asset shape, retry / blocked timers, market-selection
//! rule) are owned by the per-family binding modules under
//! `crate::bolt_v3_market_families`; `validate_strategies` dispatches
//! the strategy envelope's raw `[target]` value through
//! `crate::bolt_v3_market_families::validate_strategy_target`. Strategy-
//! archetype-specific rules (required reference-data roles, allowed
//! `[parameters.entry_order]` / `[parameters.exit_order]` combinations,
//! archetype-specific error wording) are owned by the per-archetype
//! binding modules under `crate::bolt_v3_archetypes`; those modules also
//! own archetype parameter bounds such as parameter decimal syntax and
//! root-cap comparison. `validate_strategies` dispatches into the
//! matching archetype validator via
//! `crate::bolt_v3_archetypes::validate_strategy_archetype_with_bindings`,
//! passing the production binding list from
//! `crate::strategy_bindings::production_validation_bindings`.
//! Per-provider venue-block validation (provider-shaped
//! `[clients.<id>.{data,execution,secrets}]` rules: typed
//! deserialization, cross-block presence rules, provider data /
//! execution bounds, EVM funder-address syntax, provider secret-path
//! ownership) is owned by the per-provider binding modules under
//! `crate::bolt_v3_providers`; `validate_clients_block` dispatches each
//! client block through `crate::bolt_v3_providers::validate_client_block`.
//! Only the genuinely provider-neutral SSM parameter-path utility
//! (`validate_ssm_parameter_path`) stays in this module and is exposed
//! `pub(crate)` so the per-provider secret validators can call it the
//! same way the archetype binding calls `parse_decimal_string`.

use std::{
    collections::{BTreeMap, BTreeSet, HashSet},
    path::Path,
    str::FromStr,
};

use nautilus_model::{
    enums::{BarAggregation, BarIntervalType},
    identifiers::{AccountId, ClientOrderId, InstrumentId},
};
use rust_decimal::Decimal;

use crate::bolt_v3_config::{
    AwsBlock, BoltV3RootConfig, BoltV3StrategyConfig, CHAINLINK_DATA_STREAMS_PROVIDER_KIND,
    CapitalPoolBlock, ClientBlock, DataClientReadinessProbeQuoteTargetSource,
    GATE_PROVIDER_CAPABILITIES, GATE_PROVIDER_KINDS, GateProviderBlock, GateProviderFreshnessBlock,
    KillSwitchCancelConfigBlock, KillSwitchConfigBlock, KillSwitchFlattenConfigBlock,
    KillSwitchFlattenRouteKindConfig, LoadedStrategy, NautilusBlock, PRICE_GATE_VALUE_KIND,
    PersistenceBlock, RealizedVolatilityAggregationBlock, RealizedVolatilityJumpPolicyBlock,
    RealizedVolatilityNoiseMethodBlock, RealizedVolatilityPricingComponentBlock,
    RealizedVolatilitySampleKindBlock, RealizedVolatilitySourceClassBlock, RiskBlock,
    SSM_CREDENTIAL_PARAMETER_FIELD, TEST_DOUBLE_PROVIDER_KIND,
};
use crate::bolt_v3_current_evidence::{
    CanonicalRelativeEvidencePath, PositiveFiniteEvidenceReadCap,
};
use crate::bolt_v3_kill_switch_cancel::BoltV3KillSwitchOutstandingOrderRiskSurface;
use crate::bolt_v3_loss_halt_actions::LossGovernorTradingStateAction;
use crate::bolt_v3_numeric::{
    HALF_F64, UNIT_F64, ZERO_F64, is_positive_finite, is_sha256_hex_digest,
};
use crate::bolt_v3_order_intent::{NtOrderTemplateConfig, check_nt_order_template_config};
use crate::bolt_v3_providers::{
    ReferencePriceIdentifierKind, reference_price_provider_identifier_is_configured,
    reference_price_provider_metadata,
};
use crate::bolt_v3_reference_price::reference_price_source_is_unsupported;

mod capital;
mod chainlink_data_streams;
mod clients;
mod error;
mod gate_providers;
mod kill_switch;
mod nt_blocks;
mod oms_capability;
mod persistence;
mod rate_limit;
mod reference_price;
mod risk;
mod strategy_envelope;
mod vol_sources;

use capital::validate_capital_pools;
use chainlink_data_streams::{
    validate_chainlink_feed_binding_coverage, validate_root_owned_chainlink_feed_catalog,
};
use clients::{validate_aws_block, validate_clients_block};
use gate_providers::validate_gate_providers;
use kill_switch::validate_kill_switch_block;
pub(crate) use kill_switch::validate_loaded_kill_switch_flatten;
use nt_blocks::validate_nautilus_block;
use oms_capability::validate_oms_venue_position_identity_capabilities;
use persistence::{validate_nt_reconciliation_authority, validate_persistence_block};
use rate_limit::validate_order_rate_within_venue_egress;
use reference_price::validate_reference_current_price;
use risk::validate_risk_block;
use strategy_envelope::{
    validate_shadow_order_execution_mode_forbids_managed_venue_actions,
    validate_target_gate_provider_references,
};
use vol_sources::validate_realized_volatility_surfaces;

pub(crate) use chainlink_data_streams::{
    client_with_root_chainlink_feed_catalog, resolve_chainlink_report_endpoint_url,
    validate_chainlink_report_endpoint_path, validate_https_rest_base_url,
};
pub(crate) use clients::validate_ssm_parameter_path;
pub use error::BoltV3ValidationError;
pub(crate) use rate_limit::validate_rate_limit_string;
pub(crate) use vol_sources::{
    validate_iv_source_clients, validate_realized_volatility_source_clients,
};

pub const SUPPORTED_ROOT_SCHEMA_VERSION: u32 = 2;
pub const SUPPORTED_STRATEGY_SCHEMA_VERSION: u32 = 2;
const CHAINLINK_DATA_STREAMS_FEED_BINDINGS_FIELD: &str = "feed_bindings";
const CHAINLINK_DATA_STREAMS_ENDPOINT_ID_FIELD: &str = "endpoint_id";
// Shared with the provider-owned F3 client/gate-provider consistency check
// (`crate::bolt_v3_providers::chainlink::validate_client_gate_provider_consistency`),
// which reaches them through this single core definition rather than re-declaring
// the gate-provider field names.
pub(crate) const CHAINLINK_DATA_STREAMS_REST_BASE_URL_FIELD: &str = "rest_base_url";
pub(crate) const CHAINLINK_DATA_STREAMS_REPORT_ENDPOINT_PATH_FIELD: &str = "report_endpoint_path";
pub(crate) const CHAINLINK_DATA_STREAMS_HTTP_TIMEOUT_SECS_FIELD: &str = "http_timeout_secs";
pub(crate) const CHAINLINK_DATA_STREAMS_API_KEY_SSM_PARAMETER_FIELD: &str = "api_key_ssm_parameter";
pub(crate) const CHAINLINK_DATA_STREAMS_API_SECRET_SSM_PARAMETER_FIELD: &str =
    "api_secret_ssm_parameter";
const CHAINLINK_DATA_STREAMS_OLD_SSM_CREDENTIAL_PARAMETER_FIELD: &str = "ssm_credential_parameter";
const CHAINLINK_DATA_STREAMS_RESOLUTION_IDENTITY_FIELD: &str = "resolution_identity";
const CHAINLINK_DATA_STREAMS_VALUE_KIND_FIELD: &str = "value_kind";
const CHAINLINK_DATA_STREAMS_FEED_ID_FIELD: &str = "feed_id";
const CHAINLINK_DATA_STREAMS_REPORT_SCHEMA_VERSION_FIELD: &str = "report_schema_version";
const CHAINLINK_DATA_STREAMS_REPORT_DECIMAL_SCALE_FIELD: &str = "report_decimal_scale";
const MISSING_REALIZED_VOLATILITY_SUBSAMPLE_COUNT: usize = usize::MIN;
const CHAINLINK_DATA_STREAMS_OLD_PROVIDER_LEVEL_FEED_FIELDS: &[&str] = &[
    CHAINLINK_DATA_STREAMS_FEED_ID_FIELD,
    CHAINLINK_DATA_STREAMS_REPORT_SCHEMA_VERSION_FIELD,
    CHAINLINK_DATA_STREAMS_REPORT_DECIMAL_SCALE_FIELD,
];
const CHAINLINK_DATA_STREAMS_PROVIDER_FIELDS: &[&str] = &[
    CHAINLINK_DATA_STREAMS_ENDPOINT_ID_FIELD,
    CHAINLINK_DATA_STREAMS_REST_BASE_URL_FIELD,
    CHAINLINK_DATA_STREAMS_REPORT_ENDPOINT_PATH_FIELD,
    CHAINLINK_DATA_STREAMS_HTTP_TIMEOUT_SECS_FIELD,
    CHAINLINK_DATA_STREAMS_API_KEY_SSM_PARAMETER_FIELD,
    CHAINLINK_DATA_STREAMS_API_SECRET_SSM_PARAMETER_FIELD,
    CHAINLINK_DATA_STREAMS_FEED_BINDINGS_FIELD,
    CHAINLINK_DATA_STREAMS_OLD_SSM_CREDENTIAL_PARAMETER_FIELD,
];
const TARGET_GATE_SUBSCRIPTIONS_FIELD: &str = "gate_subscriptions";
const TARGET_MARKET_MAPPINGS_FIELD: &str = "market_mappings";
const TARGET_RESOLUTION_KIND_FIELD: &str = "resolution_kind";
const TARGET_PROVIDER_ID_FIELD: &str = "provider_id";
const TARGET_PROVIDER_PREFERENCE_FIELD: &str = "provider_preference";
const TARGET_ALLOWED_PROVIDER_IDS_FIELD: &str = "allowed_provider_ids";

pub fn validate_root_only(root: &BoltV3RootConfig) -> Vec<String> {
    let mut errors = Vec::new();

    if root.schema_version != SUPPORTED_ROOT_SCHEMA_VERSION {
        errors.push(format!(
            "root schema_version={} is unsupported by this build (only {} is currently supported)",
            root.schema_version, SUPPORTED_ROOT_SCHEMA_VERSION
        ));
    }
    if root.strategy_files.is_empty() {
        errors.push("strategy_files must list at least one strategy file".to_string());
    }
    // FINDING-1: NT's `Environment` has Backtest/Sandbox/Live; bolt-v3 is a
    // live-trading LiveNode wrapper and must reject the other variants
    // explicitly rather than booting NT's kernel in an unsupported mode.
    if root.runtime.mode != nautilus_common::enums::Environment::Live {
        errors.push(format!(
            "runtime.mode `{:?}` is not supported by bolt-v3 (only Live is implemented)",
            root.runtime.mode
        ));
    }
    errors.extend(validate_nautilus_block(&root.nautilus));
    errors.extend(validate_risk_block(&root.risk));
    errors.extend(validate_order_rate_within_venue_egress(root));
    errors.extend(validate_persistence_block(&root.persistence));
    errors.extend(validate_nt_reconciliation_authority(root));
    errors.extend(crate::bolt_v3_providers::validate_reference_live_probe_block(root));
    errors.extend(validate_aws_block(&root.aws));
    errors.extend(validate_clients_block(root));
    errors.extend(validate_realized_volatility_surfaces(root));
    if let Some(gate_providers) = &root.gate_providers {
        errors.extend(validate_gate_providers(gate_providers, &root.clients));
    }
    errors.extend(crate::bolt_v3_outcome_group_sources::validate_root_sources(
        root,
    ));
    if let Some(iv) = &root.iv {
        errors.extend(crate::bolt_v3_iv::config::validate_iv_root_config(iv));
    }
    errors.extend(validate_realized_volatility_source_clients(root));
    errors.extend(validate_iv_source_clients(root));
    errors.extend(crate::bolt_v3_providers::validate_resolution_oracle_client_consistency(root));

    errors
}

pub fn validate_strategies(root: &BoltV3RootConfig, strategies: &[LoadedStrategy]) -> Vec<String> {
    let mut errors = Vec::new();
    let mut seen_instance_ids: HashSet<&str> = HashSet::new();
    let mut seen_order_id_tags: HashSet<&str> = HashSet::new();
    let mut seen_target_ids: HashSet<String> = HashSet::new();

    let default_max_notional_decimal =
        parse_decimal_string(&root.risk.default_max_notional_per_order).ok();

    for loaded in strategies {
        let context = format!("strategy `{}`", loaded.relative_path);
        let strategy = &loaded.config;

        if strategy.schema_version != SUPPORTED_STRATEGY_SCHEMA_VERSION {
            errors.push(format!(
                "{context}: schema_version={} is unsupported by this build (only {} is currently supported)",
                strategy.schema_version, SUPPORTED_STRATEGY_SCHEMA_VERSION
            ));
        }

        if !seen_instance_ids.insert(strategy.strategy_instance_id.as_str()) {
            errors.push(format!(
                "{context}: strategy_instance_id `{}` is already used by another listed strategy",
                strategy.strategy_instance_id
            ));
        }
        if !seen_order_id_tags.insert(strategy.order_id_tag.as_str()) {
            errors.push(format!(
                "{context}: order_id_tag `{}` is already used by another listed strategy",
                strategy.order_id_tag
            ));
        }

        errors.extend(
            validate_shadow_order_execution_mode_forbids_managed_venue_actions(
                &context, root, strategy,
            ),
        );
        if let Some(surface_id) = &strategy.realized_volatility_surface_id
            && !root
                .realized_volatility_surfaces
                .as_ref()
                .is_some_and(|surfaces| surfaces.contains_key(surface_id))
        {
            errors.push(format!(
                "{context}: {} `{surface_id}` references missing {}.{surface_id}",
                stringify!(realized_volatility_surface_id),
                stringify!(realized_volatility_surfaces),
            ));
        }

        if let Some(surface_id) = &strategy.realized_volatility_surface_id
            && let Some(surface) = root
                .realized_volatility_surfaces
                .as_ref()
                .and_then(|surfaces| surfaces.get(surface_id))
            && let Ok(target) =
                crate::bolt_v3_market_families::target_runtime_fields_from_target(&strategy.target)
            && target.underlying_asset != surface.canonical_base_asset
        {
            errors.push(format!(
                "{context}: realized_volatility_surface_id `{surface_id}` references realized_volatility_surfaces.{surface_id}.canonical_base_asset `{}`, but target.underlying_asset is `{}`",
                surface.canonical_base_asset, target.underlying_asset,
            ));
        }

        match root.clients.get(strategy.execution_client_id.as_str()) {
            None => errors.push(format!(
                "{context}: execution_client_id `{}` does not match any [clients.<id>] block",
                strategy.execution_client_id
            )),
            Some(client) => {
                if client.execution.is_none() {
                    errors.push(format!(
                        "{context}: strategy execution_client_id `{}` must reference an execution-capable client \
                         (the referenced client has no [execution] block)",
                        strategy.execution_client_id
                    ));
                }
            }
        }

        let (target_metadata, target_errors) =
            crate::bolt_v3_market_families::validate_strategy_target(&context, &strategy.target);
        if let Some(metadata) = target_metadata {
            let configured_target_id = metadata.configured_target_id;
            if !seen_target_ids.insert(configured_target_id.clone()) {
                errors.push(format!(
                    "{context}: configured_target_id `{configured_target_id}` is already used by another configured target"
                ));
            }
        }
        errors.extend(target_errors.into_iter().map(|error| error.to_string()));

        errors.extend(validate_reference_current_price(&context, root, strategy));
        errors.extend(
            crate::bolt_v3_archetypes::validate_strategy_archetype_with_bindings(
                &context,
                root,
                strategy,
                default_max_notional_decimal.as_ref(),
                crate::strategy_bindings::production_validation_bindings(),
            ),
        );
    }
    errors.extend(validate_target_gate_provider_references(root, strategies));
    errors.extend(validate_chainlink_feed_binding_coverage(root, strategies));
    errors.extend(validate_oms_venue_position_identity_capabilities(
        root, strategies,
    ));
    errors
}

fn string_field(table: &toml::map::Map<String, toml::Value>, field: &str) -> Option<String> {
    table
        .get(field)
        .and_then(toml::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn string_array_values(table: &toml::map::Map<String, toml::Value>, field: &str) -> Vec<String> {
    table
        .get(field)
        .and_then(toml::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(toml::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect()
}

fn first_string_array_value(
    table: &toml::map::Map<String, toml::Value>,
    field: &str,
) -> Option<String> {
    table
        .get(field)
        .and_then(toml::Value::as_array)
        .and_then(|values| values.first())
        .and_then(toml::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn single_string_array_value(
    table: &toml::map::Map<String, toml::Value>,
    field: &str,
) -> Option<String> {
    let values = table.get(field).and_then(toml::Value::as_array)?;
    (values.len() == 1)
        .then(|| values[0].as_str())
        .flatten()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

pub(crate) fn parse_decimal_string(value: &str) -> Result<Decimal, String> {
    use std::str::FromStr;
    Decimal::from_str(value).map_err(|error| error.to_string())
}
