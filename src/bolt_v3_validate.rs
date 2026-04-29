//! Startup-shaping validation for bolt-v3 root and strategy configs.
//!
//! Schema rules: docs/bolt-v3/2026-04-25-bolt-v3-schema.md Section 8.
//!
//! This module owns common strategy-envelope validation (schema
//! version, uniqueness of instance / order-id-tag, venue / execution
//! lookup, per-role reference-data structural validation), root-block
//! validation, and root risk decimal syntax only. Market-family-shaped
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
//! `crate::bolt_v3_archetypes::validate_strategy_archetype`.
//! Per-provider venue-block validation (provider-shaped
//! `[venues.<id>.{data,execution,secrets}]` rules: typed
//! deserialization, cross-block presence rules, provider data /
//! execution bounds, EVM funder-address syntax, provider secret-path
//! ownership) is owned by the per-provider binding modules under
//! `crate::bolt_v3_providers`; `validate_venues_block` dispatches each
//! venue block through `crate::bolt_v3_providers::validate_venue_block`.
//! Only the genuinely provider-neutral SSM parameter-path utility
//! (`validate_ssm_parameter_path`) stays in this module and is exposed
//! `pub(crate)` so the per-provider secret validators can call it the
//! same way the archetype binding calls `parse_decimal_string`.

use std::{collections::BTreeMap, collections::HashSet, path::Path, str::FromStr};

use nautilus_model::{
    enums::{BarAggregation, BarIntervalType},
    identifiers::{ClientId, ClientOrderId, InstrumentId},
};
use rust_decimal::Decimal;

use crate::bolt_v3_config::{
    AwsBlock, BoltV3RootConfig, BoltV3StrategyConfig, LoadedStrategy, NautilusBlock,
    PersistenceBlock, RiskBlock, VenueBlock,
};

#[derive(Debug)]
pub struct BoltV3ValidationError {
    messages: Vec<String>,
}

impl BoltV3ValidationError {
    pub fn new(messages: Vec<String>) -> Self {
        Self { messages }
    }

    pub fn messages(&self) -> &[String] {
        &self.messages
    }
}

impl std::fmt::Display for BoltV3ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(
            f,
            "bolt-v3 config validation failed ({} error{}):",
            self.messages.len(),
            if self.messages.len() == 1 { "" } else { "s" }
        )?;
        for message in &self.messages {
            writeln!(f, "  - {message}")?;
        }
        Ok(())
    }
}

impl std::error::Error for BoltV3ValidationError {}

pub const SUPPORTED_ROOT_SCHEMA_VERSION: u32 = 1;
pub const SUPPORTED_STRATEGY_SCHEMA_VERSION: u32 = 1;

pub fn validate_root_only(root: &BoltV3RootConfig) -> Vec<String> {
    let mut errors = Vec::new();

    if root.schema_version != SUPPORTED_ROOT_SCHEMA_VERSION {
        errors.push(format!(
            "root schema_version={} is unsupported by this build (only {} is currently supported)",
            root.schema_version, SUPPORTED_ROOT_SCHEMA_VERSION
        ));
    }
    if root.trader_id.trim().is_empty() {
        errors.push("trader_id must be a non-empty string".to_string());
    }
    if root.strategy_files.is_empty() {
        errors.push("strategy_files must list at least one strategy file".to_string());
    }
    errors.extend(validate_nautilus_block(&root.nautilus));
    errors.extend(validate_risk_block(&root.risk));
    errors.extend(validate_persistence_block(&root.persistence));
    errors.extend(validate_aws_block(&root.aws));
    errors.extend(validate_venues_block(&root.venues));

    errors
}

fn validate_nautilus_block(block: &NautilusBlock) -> Vec<String> {
    let mut errors = Vec::new();
    let positive_fields: &[(&str, u64)] = &[
        (
            "nautilus.timeout_connection_seconds",
            block.timeout_connection_seconds,
        ),
        (
            "nautilus.timeout_reconciliation_seconds",
            block.timeout_reconciliation_seconds,
        ),
        (
            "nautilus.timeout_portfolio_seconds",
            block.timeout_portfolio_seconds,
        ),
        (
            "nautilus.timeout_disconnection_seconds",
            block.timeout_disconnection_seconds,
        ),
        (
            "nautilus.timeout_shutdown_seconds",
            block.timeout_shutdown_seconds,
        ),
    ];
    for (label, value) in positive_fields {
        if *value == 0 {
            errors.push(format!("{label} must be a positive integer"));
        }
    }
    errors.extend(validate_data_engine_block(&block.data_engine));
    errors.extend(validate_exec_engine_block(&block.exec_engine));
    errors
}

fn validate_data_engine_block(
    block: &crate::bolt_v3_config::NautilusDataEngineBlock,
) -> Vec<String> {
    let mut errors = Vec::new();
    if let Err(error) = BarIntervalType::from_str(&block.time_bars_interval_type) {
        errors.push(format!(
            "nautilus.data_engine.time_bars_interval_type is not valid ({error}): `{}`",
            block.time_bars_interval_type
        ));
    }
    for aggregation in block.time_bars_origins.keys() {
        if let Err(error) = BarAggregation::from_str(aggregation) {
            errors.push(format!(
                "nautilus.data_engine.time_bars_origins key `{aggregation}` is not a valid Nautilus bar aggregation ({error})"
            ));
        }
    }
    for client_id in &block.external_client_ids {
        if let Err(error) = ClientId::new_checked(client_id) {
            errors.push(format!(
                "nautilus.data_engine.external_client_ids contains invalid client ID `{client_id}` ({error})"
            ));
        }
    }
    if block.graceful_shutdown_on_error {
        errors.push(
            "nautilus.data_engine.graceful_shutdown_on_error must be false; NT rejects true on the Rust live runtime"
                .to_string(),
        );
    }
    let nt_data_default = nautilus_live::config::LiveDataEngineConfig::default();
    if block.qsize != nt_data_default.qsize {
        errors.push(format!(
            "nautilus.data_engine.qsize must match NT default {}; NT rejects non-default qsize on the Rust live runtime",
            nt_data_default.qsize
        ));
    }
    errors
}

fn validate_exec_engine_block(
    block: &crate::bolt_v3_config::NautilusExecEngineBlock,
) -> Vec<String> {
    let mut errors = Vec::new();
    let positive_fields: &[(&str, u64)] = &[
        (
            "nautilus.exec_engine.inflight_check_threshold_milliseconds",
            block.inflight_check_threshold_milliseconds as u64,
        ),
        (
            "nautilus.exec_engine.open_check_threshold_milliseconds",
            block.open_check_threshold_milliseconds as u64,
        ),
        (
            "nautilus.exec_engine.max_single_order_queries_per_cycle",
            block.max_single_order_queries_per_cycle as u64,
        ),
        (
            "nautilus.exec_engine.position_check_threshold_milliseconds",
            block.position_check_threshold_milliseconds as u64,
        ),
    ];
    for (label, value) in positive_fields {
        if *value == 0 {
            errors.push(format!("{label} must be a positive integer"));
        }
    }

    if block.snapshot_orders {
        errors.push(
            "nautilus.exec_engine.snapshot_orders must be false; NT rejects true on the Rust live runtime".to_string(),
        );
    }
    if block.snapshot_positions {
        errors.push(
            "nautilus.exec_engine.snapshot_positions must be false; NT rejects true on the Rust live runtime".to_string(),
        );
    }
    if block.purge_from_database {
        errors.push(
            "nautilus.exec_engine.purge_from_database must be false; NT rejects true on the Rust live runtime".to_string(),
        );
    }
    if block.graceful_shutdown_on_error {
        errors.push(
            "nautilus.exec_engine.graceful_shutdown_on_error must be false; NT rejects true on the Rust live runtime".to_string(),
        );
    }
    let nt_exec_default = nautilus_live::config::LiveExecEngineConfig::default();
    if block.qsize != nt_exec_default.qsize {
        errors.push(format!(
            "nautilus.exec_engine.qsize must match NT default {}; NT rejects non-default qsize on the Rust live runtime",
            nt_exec_default.qsize
        ));
    }

    for client_id in &block.external_client_ids {
        if let Err(error) = ClientId::new_checked(client_id) {
            errors.push(format!(
                "nautilus.exec_engine.external_client_ids contains invalid client ID `{client_id}` ({error})"
            ));
        }
    }
    for instrument_id in &block.reconciliation_instrument_ids {
        if let Err(error) = InstrumentId::from_str(instrument_id) {
            errors.push(format!(
                "nautilus.exec_engine.reconciliation_instrument_ids contains invalid instrument ID `{instrument_id}` ({error})"
            ));
        }
    }
    for client_order_id in &block.filtered_client_order_ids {
        if let Err(error) = ClientOrderId::new_checked(client_order_id) {
            errors.push(format!(
                "nautilus.exec_engine.filtered_client_order_ids contains invalid client order ID `{client_order_id}` ({error})"
            ));
        }
    }
    errors
}

fn validate_risk_block(block: &RiskBlock) -> Vec<String> {
    let mut errors = Vec::new();
    if let Err(reason) = parse_decimal_string(&block.default_max_notional_per_order) {
        errors.push(format!(
            "risk.default_max_notional_per_order is not a valid decimal string ({reason}): `{value}`",
            value = block.default_max_notional_per_order
        ));
    }
    if block.nt_bypass {
        errors.push("risk.nt_bypass must be false".to_string());
    }
    if block.nt_graceful_shutdown_on_error {
        errors.push(
            "risk.nt_graceful_shutdown_on_error must be false; NT rejects true on the Rust live runtime"
                .to_string(),
        );
    }
    let nt_risk_default = nautilus_live::config::LiveRiskEngineConfig::default();
    if block.nt_qsize != nt_risk_default.qsize {
        errors.push(format!(
            "risk.nt_qsize must match NT default {}; NT rejects non-default qsize on the Rust live runtime",
            nt_risk_default.qsize
        ));
    }
    for (label, value) in [
        (
            "risk.nt_max_order_submit_rate",
            block.nt_max_order_submit_rate.as_str(),
        ),
        (
            "risk.nt_max_order_modify_rate",
            block.nt_max_order_modify_rate.as_str(),
        ),
    ] {
        if let Err(reason) = validate_rate_limit_string(value) {
            errors.push(format!(
                "{label} is not a valid Nautilus rate limit ({reason}): `{value}`"
            ));
        }
    }
    for (instrument_id, notional) in &block.nt_max_notional_per_order {
        // Mirrors NT's `LiveRiskEngineConfig::validate_runtime_support`;
        // keep this early-bound config validation aligned on pin bumps.
        if let Err(error) = InstrumentId::from_str(instrument_id) {
            errors.push(format!(
                "risk.nt_max_notional_per_order key `{instrument_id}` is not a valid Nautilus instrument ID ({error})"
            ));
        }
        match parse_decimal_string(notional) {
            Ok(value) if value <= Decimal::ZERO => {
                errors.push(format!(
                    "risk.nt_max_notional_per_order[`{instrument_id}`] must be a positive decimal string: `{notional}`"
                ));
            }
            Ok(_) => {}
            Err(reason) => {
                errors.push(format!(
                    "risk.nt_max_notional_per_order[`{instrument_id}`] is not a valid decimal string ({reason}): `{notional}`"
                ));
            }
        }
    }
    errors
}

fn validate_rate_limit_string(value: &str) -> Result<(), String> {
    let (limit, interval) = value
        .split_once('/')
        .ok_or_else(|| "expected `limit/HH:MM:SS`".to_string())?;
    let limit = limit.parse::<usize>().map_err(|error| error.to_string())?;
    if limit == 0 {
        return Err("limit must be greater than zero".to_string());
    }

    let mut parts = interval.split(':');
    let mut next_part = |label: &str| -> Result<u64, String> {
        parts
            .next()
            .ok_or_else(|| format!("missing {label} component"))?
            .parse::<u64>()
            .map_err(|error| format!("{label}: {error}"))
    };
    let hours = next_part("hours")?;
    let minutes = next_part("minutes")?;
    let seconds = next_part("seconds")?;
    if parts.next().is_some() {
        return Err("expected `limit/HH:MM:SS`".to_string());
    }
    if minutes >= 60 {
        return Err("minutes must be less than 60".to_string());
    }
    if seconds >= 60 {
        return Err("seconds must be less than 60".to_string());
    }
    if hours == 0 && minutes == 0 && seconds == 0 {
        return Err("interval must be greater than zero".to_string());
    }

    Ok(())
}

fn validate_persistence_block(block: &PersistenceBlock) -> Vec<String> {
    let mut errors = Vec::new();
    if !Path::new(&block.catalog_directory).is_absolute() {
        errors.push(format!(
            "persistence.catalog_directory must be an absolute path: `{}`",
            block.catalog_directory
        ));
    }
    if block.streaming.flush_interval_milliseconds == 0 {
        errors.push(
            "persistence.streaming.flush_interval_milliseconds must be a positive integer"
                .to_string(),
        );
    }
    errors
}

fn validate_aws_block(block: &AwsBlock) -> Vec<String> {
    let mut errors = Vec::new();
    if block.region.trim().is_empty() {
        errors.push("aws.region must be a non-empty string".to_string());
    }
    errors
}

fn validate_venues_block(venues: &BTreeMap<String, VenueBlock>) -> Vec<String> {
    let mut errors = Vec::new();
    if venues.is_empty() {
        errors.push("venues must define at least one venue block".to_string());
        return errors;
    }
    // The current bolt-v3 scope is one venue per provider key. Multi-venue
    // routing (multiple keyed venues for the same provider) is not yet
    // covered by the NT typed-venue routing path or by bolt-v3 strategy
    // validation. NT client registration names can differ, but engine
    // instrument subscriptions still key on typed venues such as
    // POLYMARKET/BINANCE, so we fail closed until that routing is
    // explicitly designed.
    let mut kind_counts: BTreeMap<String, Vec<&str>> = BTreeMap::new();
    for (key, venue) in venues {
        kind_counts
            .entry(venue.kind.as_str().to_string())
            .or_default()
            .push(key.as_str());
    }
    for (kind, keys) in &kind_counts {
        if keys.len() > 1 {
            errors.push(format!(
                "venues: at most one [venues.<id>] block per kind is supported in this slice; \
                 kind `{kind}` is declared by {} venues: {}",
                keys.len(),
                keys.join(", ")
            ));
        }
    }
    for (key, venue) in venues {
        errors.extend(crate::bolt_v3_providers::validate_venue_block(key, venue));
    }
    errors
}

/// Provider-neutral SSM parameter-path utility shared by the per-
/// provider secret validators in `crate::bolt_v3_providers`. Stays in
/// core because the path-shape rule itself is provider-neutral and is
/// also the gate behind the SSM-only invariant; mirrors the cross-
/// layer call that the archetype binding makes into
/// `parse_decimal_string`.
pub(crate) fn validate_ssm_parameter_path(key: &str, field: &str, value: &str) -> Vec<String> {
    let mut errors = Vec::new();
    let trimmed = value.trim();
    if trimmed.is_empty() {
        errors.push(format!(
            "venues.{key}.secrets.{field} must be a non-empty SSM path"
        ));
    } else if !trimmed.starts_with('/') {
        // The Rust AWS SDK accepts both `name`-style and `/name`-style
        // parameter references, but bolt-v3 standardizes on
        // absolute-style hierarchical paths so an SSM resource layout
        // like `/bolt/<venue>/<field>` is the only supported shape and
        // typos that drop the leading slash fail closed at startup.
        errors.push(format!(
            "venues.{key}.secrets.{field} must be an absolute-style SSM parameter path starting with `/`: `{value}`"
        ));
    }
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

        match root.venues.get(&strategy.venue) {
            None => errors.push(format!(
                "{context}: venue reference `{}` does not match any [venues.<id>] block",
                strategy.venue
            )),
            Some(venue) => {
                if venue.execution.is_none() {
                    errors.push(format!(
                        "{context}: strategy venue `{}` must reference an execution-capable venue \
                         (the referenced venue has no [execution] block)",
                        strategy.venue
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
        errors.extend(target_errors);

        errors.extend(validate_reference_data(&context, root, strategy));
        errors.extend(crate::bolt_v3_archetypes::validate_strategy_archetype(
            &context,
            strategy,
            default_max_notional_decimal.as_ref(),
        ));
    }

    errors
}

fn validate_reference_data(
    context: &str,
    root: &BoltV3RootConfig,
    strategy: &BoltV3StrategyConfig,
) -> Vec<String> {
    let mut errors = Vec::new();

    for (role, block) in &strategy.reference_data {
        match root.venues.get(&block.venue) {
            None => errors.push(format!(
                "{context}: reference_data.{role}.venue `{}` does not match any [venues.<id>] block",
                block.venue
            )),
            Some(venue) => {
                if venue.data.is_none() {
                    errors.push(format!(
                        "{context}: reference_data.{role}.venue `{}` must reference a data-capable venue \
                         (the referenced venue has no [data] block)",
                        block.venue
                    ));
                }
            }
        }
        if block.instrument_id.trim().is_empty() {
            errors.push(format!(
                "{context}: reference_data.{role}.instrument_id must not be empty"
            ));
        }
    }

    errors
}

pub(crate) fn parse_decimal_string(value: &str) -> Result<Decimal, String> {
    use std::str::FromStr;
    Decimal::from_str(value).map_err(|error| error.to_string())
}
