//! Startup-shaping validation for bolt-v3 root and strategy configs.
//!
//! Schema rules: docs/bolt-v3/2026-04-25-bolt-v3-schema.md Section 8.
//! Cadence slug-token table: docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md Section 5.3.

use std::{collections::BTreeMap, collections::HashSet, path::Path};

use rust_decimal::Decimal;

use crate::bolt_v3_config::{
    ArchetypeOrderType, ArchetypeTimeInForce, AwsBlock, BinanceDataConfig, BinanceSecretsConfig,
    BoltV3RootConfig, BoltV3StrategyConfig, LoadedStrategy, LoggingBlock, NautilusBlock,
    OrderParams, PersistenceBlock, PolymarketDataConfig, PolymarketExecutionConfig,
    PolymarketSecretsConfig, RiskBlock, StrategyArchetype, TargetBlock, VenueBlock, VenueKind,
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

const UPDOWN_CADENCE_SLUG_TOKEN_TABLE: &[(i64, &str)] = &[
    (60, "1m"),
    (300, "5m"),
    (900, "15m"),
    (3600, "1h"),
    (14400, "4h"),
];

pub fn updown_cadence_slug_token(cadence_seconds: i64) -> Option<&'static str> {
    UPDOWN_CADENCE_SLUG_TOKEN_TABLE
        .iter()
        .find_map(|(seconds, token)| (*seconds == cadence_seconds).then_some(*token))
}

pub fn supported_updown_cadence_seconds() -> Vec<i64> {
    UPDOWN_CADENCE_SLUG_TOKEN_TABLE
        .iter()
        .map(|(seconds, _)| *seconds)
        .collect()
}

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
    errors.extend(validate_logging_block(&root.logging));
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
    errors
}

fn validate_risk_block(block: &RiskBlock) -> Vec<String> {
    let mut errors = Vec::new();
    let positive_fields: &[(&str, u64)] = &[
        ("risk.max_order_submit_count", block.max_order_submit_count),
        (
            "risk.max_order_submit_interval_seconds",
            block.max_order_submit_interval_seconds,
        ),
        ("risk.max_order_modify_count", block.max_order_modify_count),
        (
            "risk.max_order_modify_interval_seconds",
            block.max_order_modify_interval_seconds,
        ),
    ];
    for (label, value) in positive_fields {
        if *value == 0 {
            errors.push(format!("{label} must be a positive integer"));
        }
    }
    if let Err(reason) = parse_decimal_string(&block.default_max_notional_per_order) {
        errors.push(format!(
            "risk.default_max_notional_per_order is not a valid decimal string ({reason}): `{value}`",
            value = block.default_max_notional_per_order
        ));
    }
    errors
}

fn validate_logging_block(block: &LoggingBlock) -> Vec<String> {
    let mut errors = Vec::new();
    if !Path::new(&block.log_directory).is_absolute() {
        errors.push(format!(
            "logging.log_directory must be an absolute path: `{}`",
            block.log_directory
        ));
    }
    errors
}

fn validate_persistence_block(block: &PersistenceBlock) -> Vec<String> {
    let mut errors = Vec::new();
    if !Path::new(&block.state_directory).is_absolute() {
        errors.push(format!(
            "persistence.state_directory must be an absolute path: `{}`",
            block.state_directory
        ));
    }
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
    for (key, venue) in venues {
        match venue.kind {
            VenueKind::Polymarket => errors.extend(validate_polymarket_venue(key, venue)),
            VenueKind::Binance => errors.extend(validate_binance_venue(key, venue)),
        }
    }
    errors
}

fn validate_polymarket_venue(key: &str, venue: &VenueBlock) -> Vec<String> {
    let mut errors = Vec::new();
    if let Some(data) = &venue.data {
        match data.clone().try_into::<PolymarketDataConfig>() {
            Ok(parsed) => errors.extend(validate_polymarket_data_bounds(key, &parsed)),
            Err(message) => errors.push(format!("venues.{key}.data: {message}")),
        }
    }
    if let Some(execution) = &venue.execution {
        match execution.clone().try_into::<PolymarketExecutionConfig>() {
            Ok(parsed) => {
                if parsed.account_id.trim().is_empty() {
                    errors.push(format!(
                        "venues.{key}.execution.account_id must be a non-empty string"
                    ));
                }
                if parsed.funder_address.trim().is_empty() {
                    errors.push(format!(
                        "venues.{key}.execution.funder_address must be a non-empty public address"
                    ));
                }
                errors.extend(validate_polymarket_execution_bounds(key, &parsed));
            }
            Err(message) => {
                errors.push(format!("venues.{key}.execution: {message}"));
            }
        }
        // Per docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md Section 3,
        // every Polymarket execution venue must resolve credentials through
        // SSM. The secret-block requirement is what guarantees the env-var
        // blocklist actually fires for that venue at startup; without it an
        // operator could declare [execution] alone and silently bypass the
        // SSM-only invariant by reading the legacy POLYMARKET_* env vars.
        if venue.secrets.is_none() {
            errors.push(format!(
                "venues.{key} (kind=polymarket) declares [execution] but is missing the required [secrets] block; \
                 the bolt-v3 secret contract requires SSM credential resolution for every Polymarket execution venue"
            ));
        }
    }
    if let Some(secrets) = &venue.secrets {
        match secrets.clone().try_into::<PolymarketSecretsConfig>() {
            Ok(parsed) => errors.extend(validate_polymarket_secret_paths(key, &parsed)),
            Err(message) => errors.push(format!("venues.{key}.secrets: {message}")),
        }
    }
    errors
}

fn validate_binance_venue(key: &str, venue: &VenueBlock) -> Vec<String> {
    let mut errors = Vec::new();
    if venue.execution.is_some() {
        errors.push(format!(
            "venues.{key} (kind=binance) is not allowed to declare an [execution] block in the current bolt-v3 scope"
        ));
    }
    if let Some(data) = &venue.data {
        if let Err(message) = data.clone().try_into::<BinanceDataConfig>() {
            errors.push(format!("venues.{key}.data: {message}"));
        }
        // Per docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md Section 3
        // and docs/bolt-v3/2026-04-25-bolt-v3-schema.md Section 5, every
        // Binance reference-data venue must resolve credentials through SSM.
        // Mirror the Polymarket-execution rule above: the secret-block
        // requirement is the gate that makes the env-var blocklist effective.
        if venue.secrets.is_none() {
            errors.push(format!(
                "venues.{key} (kind=binance) declares [data] but is missing the required [secrets] block; \
                 the bolt-v3 secret contract requires SSM credential resolution for every Binance reference-data venue"
            ));
        }
    }
    if let Some(secrets) = &venue.secrets {
        match secrets.clone().try_into::<BinanceSecretsConfig>() {
            Ok(parsed) => errors.extend(validate_binance_secret_paths(key, &parsed)),
            Err(message) => errors.push(format!("venues.{key}.secrets: {message}")),
        }
    }
    errors
}

fn validate_polymarket_data_bounds(key: &str, data: &PolymarketDataConfig) -> Vec<String> {
    let mut errors = Vec::new();
    let positive_fields: &[(&str, u64)] = &[
        ("http_timeout_seconds", data.http_timeout_seconds),
        ("ws_timeout_seconds", data.ws_timeout_seconds),
        (
            "update_instruments_interval_minutes",
            data.update_instruments_interval_minutes,
        ),
        (
            "websocket_max_subscriptions_per_connection",
            data.websocket_max_subscriptions_per_connection,
        ),
    ];
    for (field, value) in positive_fields {
        if *value == 0 {
            errors.push(format!(
                "venues.{key}.data.{field} must be a positive integer"
            ));
        }
    }
    errors
}

fn validate_polymarket_execution_bounds(
    key: &str,
    execution: &PolymarketExecutionConfig,
) -> Vec<String> {
    let mut errors = Vec::new();
    let positive_fields: &[(&str, u64)] = &[
        ("http_timeout_seconds", execution.http_timeout_seconds),
        ("max_retries", execution.max_retries),
        (
            "retry_delay_initial_milliseconds",
            execution.retry_delay_initial_milliseconds,
        ),
        (
            "retry_delay_max_milliseconds",
            execution.retry_delay_max_milliseconds,
        ),
        ("ack_timeout_seconds", execution.ack_timeout_seconds),
    ];
    for (field, value) in positive_fields {
        if *value == 0 {
            errors.push(format!(
                "venues.{key}.execution.{field} must be a positive integer"
            ));
        }
    }
    if execution.retry_delay_initial_milliseconds > execution.retry_delay_max_milliseconds {
        errors.push(format!(
            "venues.{key}.execution.retry_delay_initial_milliseconds ({}) must be <= retry_delay_max_milliseconds ({})",
            execution.retry_delay_initial_milliseconds, execution.retry_delay_max_milliseconds
        ));
    }
    errors
}

fn validate_polymarket_secret_paths(key: &str, secrets: &PolymarketSecretsConfig) -> Vec<String> {
    let mut errors = Vec::new();
    let path_fields: &[(&str, &str)] = &[
        ("private_key_ssm_path", &secrets.private_key_ssm_path),
        ("api_key_ssm_path", &secrets.api_key_ssm_path),
        ("api_secret_ssm_path", &secrets.api_secret_ssm_path),
        ("passphrase_ssm_path", &secrets.passphrase_ssm_path),
    ];
    for (field, value) in path_fields {
        if value.trim().is_empty() {
            errors.push(format!(
                "venues.{key}.secrets.{field} must be a non-empty SSM path"
            ));
        }
    }
    errors
}

fn validate_binance_secret_paths(key: &str, secrets: &BinanceSecretsConfig) -> Vec<String> {
    let mut errors = Vec::new();
    let path_fields: &[(&str, &str)] = &[
        ("api_key_ssm_path", &secrets.api_key_ssm_path),
        ("api_secret_ssm_path", &secrets.api_secret_ssm_path),
    ];
    for (field, value) in path_fields {
        if value.trim().is_empty() {
            errors.push(format!(
                "venues.{key}.secrets.{field} must be a non-empty SSM path"
            ));
        }
    }
    errors
}

pub fn validate_strategies(root: &BoltV3RootConfig, strategies: &[LoadedStrategy]) -> Vec<String> {
    let mut errors = Vec::new();

    let mut seen_instance_ids: HashSet<&str> = HashSet::new();
    let mut seen_order_id_tags: HashSet<&str> = HashSet::new();
    let mut seen_target_ids: HashSet<&str> = HashSet::new();

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
        if !seen_target_ids.insert(strategy.target.configured_target_id.as_str()) {
            errors.push(format!(
                "{context}: configured_target_id `{}` is already used by another configured target",
                strategy.target.configured_target_id
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

        errors.extend(validate_target(&context, &strategy.target));
        errors.extend(validate_reference_data(&context, root, strategy));
        errors.extend(validate_archetype_parameters(
            &context,
            strategy,
            default_max_notional_decimal.as_ref(),
        ));
    }

    errors
}

fn validate_target(context: &str, target: &TargetBlock) -> Vec<String> {
    let mut errors = Vec::new();

    let underlying = target.underlying_asset.as_str();
    if underlying.is_empty() {
        errors.push(format!(
            "{context}: target.underlying_asset must not be empty"
        ));
    } else if underlying.chars().count() > 32 {
        errors.push(format!(
            "{context}: target.underlying_asset must be 1-32 characters (got {})",
            underlying.chars().count()
        ));
    } else if !underlying
        .chars()
        .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
    {
        errors.push(format!(
            "{context}: target.underlying_asset must use only uppercase ASCII letters, digits, and underscores (got `{underlying}`)"
        ));
    }

    if target.cadence_seconds <= 0 {
        errors.push(format!(
            "{context}: target.cadence_seconds must be a positive integer (got {})",
            target.cadence_seconds
        ));
    } else if target.cadence_seconds % 60 != 0 {
        errors.push(format!(
            "{context}: target.cadence_seconds must be divisible by 60 (got {})",
            target.cadence_seconds
        ));
    } else if updown_cadence_slug_token(target.cadence_seconds).is_none() {
        let supported = supported_updown_cadence_seconds();
        errors.push(format!(
            "{context}: target.cadence_seconds={} has no runtime-contract-defined updown slug-token mapping; supported values are {:?}",
            target.cadence_seconds, supported
        ));
    }

    if target.retry_interval_seconds == 0 {
        errors.push(format!(
            "{context}: target.retry_interval_seconds must be a positive integer"
        ));
    }
    if target.blocked_after_seconds == 0 {
        errors.push(format!(
            "{context}: target.blocked_after_seconds must be a positive integer"
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

    if matches!(
        strategy.strategy_archetype,
        StrategyArchetype::BinaryOracleEdgeTaker
    ) && !strategy.reference_data.contains_key("primary")
    {
        errors.push(format!(
            "{context}: strategy_archetype `binary_oracle_edge_taker` requires [reference_data.primary]"
        ));
    }

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

fn validate_archetype_parameters(
    context: &str,
    strategy: &BoltV3StrategyConfig,
    default_max_notional: Option<&Decimal>,
) -> Vec<String> {
    let mut errors = Vec::new();

    match strategy.strategy_archetype {
        StrategyArchetype::BinaryOracleEdgeTaker => {
            errors.extend(check_binary_oracle_entry_order_combination(
                context,
                &strategy.parameters.entry_order,
            ));
            errors.extend(check_binary_oracle_exit_order_combination(
                context,
                &strategy.parameters.exit_order,
            ));
        }
    }

    let order_target_decimal = match parse_decimal_string(
        &strategy.parameters.order_notional_target,
    ) {
        Ok(value) => Some(value),
        Err(reason) => {
            errors.push(format!(
                    "{context}: parameters.order_notional_target is not a valid decimal string ({reason}): `{}`",
                    strategy.parameters.order_notional_target
                ));
            None
        }
    };
    if let Err(reason) = parse_decimal_string(&strategy.parameters.maximum_position_notional) {
        errors.push(format!(
            "{context}: parameters.maximum_position_notional is not a valid decimal string ({reason}): `{}`",
            strategy.parameters.maximum_position_notional
        ));
    }
    if let (Some(order_target), Some(default_max)) =
        (order_target_decimal.as_ref(), default_max_notional)
    {
        if order_target > default_max {
            errors.push(format!(
                "{context}: parameters.order_notional_target ({order_target}) must be <= root risk.default_max_notional_per_order ({default_max})"
            ));
        }
    }

    errors
}

fn check_binary_oracle_entry_order_combination(context: &str, entry: &OrderParams) -> Vec<String> {
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

fn check_binary_oracle_exit_order_combination(context: &str, exit: &OrderParams) -> Vec<String> {
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

fn parse_decimal_string(value: &str) -> Result<Decimal, String> {
    use std::str::FromStr;
    Decimal::from_str(value).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn updown_cadence_token_table_matches_runtime_contract() {
        assert_eq!(updown_cadence_slug_token(60), Some("1m"));
        assert_eq!(updown_cadence_slug_token(300), Some("5m"));
        assert_eq!(updown_cadence_slug_token(900), Some("15m"));
        assert_eq!(updown_cadence_slug_token(3600), Some("1h"));
        assert_eq!(updown_cadence_slug_token(14400), Some("4h"));
        assert_eq!(updown_cadence_slug_token(120), None);
    }
}
