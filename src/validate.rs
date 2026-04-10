use crate::config::{Config, ReferenceConfig, ReferenceVenueKind};
use crate::live_config::{LiveLocalConfig, LiveReferenceInput};
use nautilus_model::types::Quantity;
use std::collections::{HashMap, hash_map::Entry};
use std::str::FromStr;
use toml::Value;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ValidationError {
    pub field: String,
    pub code: &'static str,
    pub message: String,
}

impl std::fmt::Display for ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.field, self.message)
    }
}

fn push_error(errors: &mut Vec<ValidationError>, field: &str, code: &'static str, message: String) {
    errors.push(ValidationError {
        field: field.to_string(),
        code,
        message,
    });
}

// ═══════════════════════════════════════════════════════════════════
// NT-contract-derived helpers
// ═══════════════════════════════════════════════════════════════════

/// Mirrors NT's `check_valid_string_ascii`: non-empty, non-whitespace-only, ASCII-only.
fn check_nt_ascii(errors: &mut Vec<ValidationError>, field: &str, value: &str) {
    if value.is_empty() {
        push_error(
            errors,
            field,
            "empty",
            "must not be empty, got \"\"".to_string(),
        );
    } else if value.trim().is_empty() {
        push_error(
            errors,
            field,
            "whitespace_only",
            format!("must not be whitespace-only, got \"{value}\""),
        );
    } else if !value.is_ascii() {
        push_error(
            errors,
            field,
            "non_ascii",
            format!("must be ASCII-only, got \"{value}\""),
        );
    }
}

fn check_non_empty(errors: &mut Vec<ValidationError>, field: &str, value: &str) {
    if value.is_empty() {
        push_error(
            errors,
            field,
            "empty",
            "must not be empty, got \"\"".to_string(),
        );
    } else if value.trim().is_empty() {
        push_error(
            errors,
            field,
            "whitespace_only",
            format!("must not be whitespace-only, got \"{value}\""),
        );
    }
}

/// Shared hyphen-check logic. `split_fn` selects the split direction to
/// match the exact NT constructor for each identifier type.
fn check_nt_hyphenated(
    errors: &mut Vec<ValidationError>,
    field: &str,
    value: &str,
    split_fn: fn(&str) -> Option<(&str, &str)>,
    example: &str,
) {
    if value.is_empty() || value.trim().is_empty() || !value.is_ascii() {
        check_nt_ascii(errors, field, value);
        return;
    }

    match split_fn(value) {
        None => push_error(
            errors,
            field,
            "missing_hyphen",
            format!(
                "must contain a hyphen separating name and tag, got \"{value}\" (example: \"{example}\")"
            ),
        ),
        Some(("", _)) => push_error(
            errors,
            field,
            "empty_name_part",
            format!("has empty name part before hyphen, got \"{value}\""),
        ),
        Some((_, "")) => push_error(
            errors,
            field,
            "empty_tag_part",
            format!("has empty tag part after hyphen, got \"{value}\""),
        ),
        Some(_) => {}
    }
}

fn split_first_hyphen(s: &str) -> Option<(&str, &str)> {
    s.split_once('-')
}

fn split_last_hyphen(s: &str) -> Option<(&str, &str)> {
    s.rsplit_once('-')
}

// ═══════════════════════════════════════════════════════════════════
// Domain-specific helpers
// ═══════════════════════════════════════════════════════════════════

/// Non-empty, no whitespace anywhere (domain rule for event slugs).
fn check_non_empty_no_whitespace(errors: &mut Vec<ValidationError>, field: &str, value: &str) {
    if value.is_empty() {
        push_error(
            errors,
            field,
            "empty",
            "must not be empty, got \"\"".to_string(),
        );
    } else if value.trim().is_empty() {
        push_error(
            errors,
            field,
            "whitespace_only",
            format!("must not be whitespace-only, got \"{value}\""),
        );
    } else if value.contains(char::is_whitespace) {
        push_error(
            errors,
            field,
            "contains_whitespace",
            format!("must not contain whitespace, got \"{value}\""),
        );
    }
}

/// NT `InstrumentId::from` uses `rsplit_once('.')` -> (symbol, venue).
/// We enforce venue == "POLYMARKET" and symbol non-empty.
fn check_instrument_id(errors: &mut Vec<ValidationError>, field: &str, value: &str) {
    if value.is_empty() {
        push_error(
            errors,
            field,
            "empty",
            "must not be empty, got \"\"".to_string(),
        );
    } else if !value.ends_with(".POLYMARKET") {
        push_error(
            errors,
            field,
            "missing_venue_suffix",
            format!(
                "must end with .POLYMARKET, got \"{value}\" (example: \"0xabc-12345.POLYMARKET\")"
            ),
        );
    } else {
        let symbol = &value[..value.len() - ".POLYMARKET".len()];
        if symbol.is_empty() {
            push_error(
                errors,
                field,
                "empty_symbol",
                format!("symbol part before .POLYMARKET must not be empty, got \"{value}\""),
            );
        } else if symbol.trim().is_empty() {
            push_error(
                errors,
                field,
                "whitespace_only_symbol",
                format!(
                    "symbol part before .POLYMARKET must not be whitespace-only, got \"{value}\""
                ),
            );
        }
    }
}

fn check_hex_prefixed(errors: &mut Vec<ValidationError>, field: &str, value: &str) {
    if value.is_empty() {
        push_error(
            errors,
            field,
            "empty",
            "must not be empty, got \"\"".to_string(),
        );
    } else if !value.starts_with("0x") {
        push_error(
            errors,
            field,
            "missing_hex_prefix",
            format!("must start with 0x, got \"{value}\" (example: \"0xabc...\")"),
        );
    }
}

/// Bolt policy: quantities must be strictly positive even though NT accepts zero.
/// Parsing delegates entirely to NT's `Quantity::from_str`.
fn check_strictly_positive_qty(errors: &mut Vec<ValidationError>, field: &str, value: &str) {
    match Quantity::from_str(value) {
        Ok(qty) if qty.raw == 0 => push_error(
            errors,
            field,
            "not_positive_number",
            format!("must be a positive number, got \"{value}\""),
        ),
        Ok(_) => {}
        Err(e) => push_error(
            errors,
            field,
            "not_parseable",
            format!("must be a valid Quantity, got \"{value}\" ({e})"),
        ),
    }
}

fn check_positive_u64(errors: &mut Vec<ValidationError>, field: &str, value: u64) {
    if value == 0 {
        push_error(
            errors,
            field,
            "not_positive",
            format!("must be > 0, got {value}"),
        );
    }
}

fn check_positive_finite_f64(errors: &mut Vec<ValidationError>, field: &str, value: f64) {
    if !value.is_finite() || value <= 0.0 {
        push_error(
            errors,
            field,
            "not_positive_finite",
            format!("must be > 0.0 and finite, got {value}"),
        );
    }
}

fn check_non_negative_finite_f64(errors: &mut Vec<ValidationError>, field: &str, value: f64) {
    if !value.is_finite() || value < 0.0 {
        push_error(
            errors,
            field,
            "not_non_negative_finite",
            format!("must be >= 0.0 and finite, got {value}"),
        );
    }
}

fn check_signature_type(errors: &mut Vec<ValidationError>, field: &str, value: i64) {
    if !(0..=2).contains(&value) {
        push_error(
            errors,
            field,
            "invalid_signature_type",
            format!("must be one of [0, 1, 2], got {value} (valid values: 0, 1, 2)"),
        );
    }
}

fn check_ssm_path(errors: &mut Vec<ValidationError>, field: &str, value: &str) {
    if value.is_empty() {
        push_error(
            errors,
            field,
            "empty",
            "must not be empty, got \"\"".to_string(),
        );
    } else if !value.starts_with('/') {
        push_error(
            errors,
            field,
            "missing_leading_slash",
            format!(
                "must be an absolute SSM path starting with /, got \"{value}\" (example: \"/bolt/poly/pk\")"
            ),
        );
    }
}

/// Allowlist check for a field that must be one of an exact set of values.
fn check_allowlist(
    errors: &mut Vec<ValidationError>,
    field: &str,
    value: &str,
    allowed: &[&str],
    code: &'static str,
) {
    if !allowed.contains(&value) {
        push_error(
            errors,
            field,
            code,
            format!("must be one of {allowed:?}, got \"{value}\""),
        );
    }
}

fn get_required_str<'a>(
    errors: &mut Vec<ValidationError>,
    table: &'a Value,
    key: &str,
    field: &str,
    code: &'static str,
) -> Option<&'a str> {
    match table.get(key) {
        None => {
            push_error(
                errors,
                field,
                code,
                "is missing required string field".to_string(),
            );
            None
        }
        Some(value) => match value.as_str() {
            Some(value) => Some(value),
            None => {
                push_error(
                    errors,
                    field,
                    "wrong_type",
                    format!("must be a string, got {} value", value.type_str()),
                );
                None
            }
        },
    }
}

fn get_required_i64(
    errors: &mut Vec<ValidationError>,
    table: &Value,
    key: &str,
    field: &str,
    code: &'static str,
) -> Option<i64> {
    match table.get(key) {
        None => {
            push_error(
                errors,
                field,
                code,
                "is missing required integer field".to_string(),
            );
            None
        }
        Some(value) => match value.as_integer() {
            Some(value) => Some(value),
            None => {
                push_error(
                    errors,
                    field,
                    "wrong_type",
                    format!("must be an integer, got {} value", value.type_str()),
                );
                None
            }
        },
    }
}

fn get_required_array<'a>(
    errors: &mut Vec<ValidationError>,
    table: &'a Value,
    key: &str,
    field: &str,
    code: &'static str,
) -> Option<&'a [Value]> {
    match table.get(key) {
        None => {
            push_error(
                errors,
                field,
                code,
                "is missing required array field".to_string(),
            );
            None
        }
        Some(value) => match value.as_array() {
            Some(value) => Some(value),
            None => {
                push_error(
                    errors,
                    field,
                    "wrong_type",
                    format!("must be an array, got {} value", value.type_str()),
                );
                None
            }
        },
    }
}

fn check_required_ssm_path_opt(
    errors: &mut Vec<ValidationError>,
    field: &str,
    value: Option<&String>,
    missing_code: &'static str,
) {
    match value {
        Some(value) => check_ssm_path(errors, field, value),
        None => push_error(
            errors,
            field,
            missing_code,
            "is missing required string field".to_string(),
        ),
    }
}

fn first_seen_index<'a>(
    seen: &mut HashMap<&'a str, usize>,
    key: &'a str,
    index: usize,
) -> Option<usize> {
    match seen.entry(key) {
        Entry::Occupied(entry) => Some(*entry.get()),
        Entry::Vacant(entry) => {
            entry.insert(index);
            None
        }
    }
}

fn implied_reference_venue_kind(resolution_basis: &str) -> Option<ReferenceVenueKind> {
    const PREFIXES: &[(&str, ReferenceVenueKind)] = &[
        ("binance_", ReferenceVenueKind::Binance),
        ("bybit_", ReferenceVenueKind::Bybit),
        ("deribit_", ReferenceVenueKind::Deribit),
        ("hyperliquid_", ReferenceVenueKind::Hyperliquid),
        ("kraken_", ReferenceVenueKind::Kraken),
        ("okx_", ReferenceVenueKind::Okx),
        ("polymarket_", ReferenceVenueKind::Polymarket),
        ("chainlink_", ReferenceVenueKind::Chainlink),
    ];

    PREFIXES
        .iter()
        .find_map(|(prefix, kind)| resolution_basis.starts_with(prefix).then(|| kind.clone()))
}

fn check_contract_path_catalog_dependency(
    errors: &mut Vec<ValidationError>,
    catalog_path: &str,
    contract_path: Option<&str>,
) {
    if let Some(contract_path) = contract_path {
        if contract_path.trim().is_empty() {
            push_error(
                errors,
                "streaming.contract_path",
                "empty",
                "streaming.contract_path must not be empty when provided".to_string(),
            );
        } else if catalog_path.trim().is_empty() {
            push_error(
                errors,
                "streaming.contract_path",
                "requires_catalog_path",
                "streaming.contract_path requires non-empty streaming.catalog_path".to_string(),
            );
        }
    }
}

fn check_live_local_contract_path_shape(
    errors: &mut Vec<ValidationError>,
    contract_path: Option<&str>,
) {
    if let Some(contract_path) = contract_path {
        if contract_path.trim().is_empty() {
            return;
        }

        if contract_path.contains("://") {
            push_error(
                errors,
                "streaming.contract_path",
                "non_local",
                format_live_local_non_local_contract_path_message(contract_path),
            );
        }
    }
}

fn format_live_local_non_local_contract_path_message(contract_path: &str) -> String {
    format!("streaming.contract_path must be a local path, got \"{contract_path}\"")
}

fn format_runtime_non_local_contract_path_message(contract_path: &str) -> String {
    format!("streaming.contract_path must be a local absolute path, got \"{contract_path}\"")
}

fn check_runtime_contract_path_shape(
    errors: &mut Vec<ValidationError>,
    contract_path: Option<&str>,
) {
    if let Some(contract_path) = contract_path {
        if contract_path.trim().is_empty() {
            return;
        }

        if contract_path.contains("://") {
            push_error(
                errors,
                "streaming.contract_path",
                "non_local",
                format_runtime_non_local_contract_path_message(contract_path),
            );
        } else if !std::path::Path::new(contract_path).is_absolute() {
            push_error(
                errors,
                "streaming.contract_path",
                "not_absolute",
                format!(
                    "streaming.contract_path must be a local absolute path, got \"{contract_path}\""
                ),
            );
        }
    }
}
// ═══════════════════════════════════════════════════════════════════
// Public validators
// ═══════════════════════════════════════════════════════════════════

const VALID_ENVIRONMENTS: &[&str] = &["Live", "Sandbox"];
const VALID_LOG_LEVELS: &[&str] = &["Trace", "Debug", "Info", "Warn", "Error", "Off"];
const VALID_DATA_CLIENT_TYPES: &[&str] = &["polymarket"];
const VALID_EXEC_CLIENT_TYPES: &[&str] = &["polymarket"];
const VALID_STRATEGY_TYPES: &[&str] = &["exec_tester"];

/// Validate a human-edited live local config before rendering.
/// Returns all validation errors found, sorted by field path for deterministic output.
pub fn validate_live_local(config: &LiveLocalConfig) -> Vec<ValidationError> {
    let mut errors = Vec::new();

    check_non_empty(&mut errors, "node.name", &config.node.name);
    check_nt_hyphenated(
        &mut errors,
        "node.trader_id",
        &config.node.trader_id,
        split_last_hyphen,
        "NAME-TAG",
    );
    check_allowlist(
        &mut errors,
        "node.environment",
        &config.node.environment,
        VALID_ENVIRONMENTS,
        "invalid_environment",
    );

    check_allowlist(
        &mut errors,
        "logging.stdout_level",
        &config.logging.stdout_level,
        VALID_LOG_LEVELS,
        "invalid_log_level",
    );
    check_allowlist(
        &mut errors,
        "logging.file_level",
        &config.logging.file_level,
        VALID_LOG_LEVELS,
        "invalid_log_level",
    );

    check_positive_u64(
        &mut errors,
        "timeouts.connection_secs",
        config.timeouts.connection_secs,
    );
    check_positive_u64(
        &mut errors,
        "timeouts.reconciliation_secs",
        config.timeouts.reconciliation_secs,
    );
    check_positive_u64(
        &mut errors,
        "timeouts.portfolio_secs",
        config.timeouts.portfolio_secs,
    );
    check_positive_u64(
        &mut errors,
        "timeouts.disconnection_secs",
        config.timeouts.disconnection_secs,
    );
    check_positive_u64(
        &mut errors,
        "timeouts.post_stop_delay_secs",
        config.timeouts.post_stop_delay_secs,
    );
    check_positive_u64(
        &mut errors,
        "timeouts.shutdown_delay_secs",
        config.timeouts.shutdown_delay_secs,
    );

    check_nt_ascii(
        &mut errors,
        "polymarket.client_name",
        &config.polymarket.client_name,
    );
    check_non_empty_no_whitespace(
        &mut errors,
        "polymarket.event_slug",
        &config.polymarket.event_slug,
    );
    check_instrument_id(
        &mut errors,
        "polymarket.instrument_id",
        &config.polymarket.instrument_id,
    );
    check_nt_hyphenated(
        &mut errors,
        "polymarket.account_id",
        &config.polymarket.account_id,
        split_first_hyphen,
        "ISSUER-ACCOUNT",
    );
    check_hex_prefixed(&mut errors, "polymarket.funder", &config.polymarket.funder);
    check_signature_type(
        &mut errors,
        "polymarket.signature_type",
        i64::from(config.polymarket.signature_type),
    );

    if config.strategy.strategy_id != "EXTERNAL" {
        check_nt_hyphenated(
            &mut errors,
            "strategy.strategy_id",
            &config.strategy.strategy_id,
            split_last_hyphen,
            "NAME-TAG",
        );
    }
    check_strictly_positive_qty(
        &mut errors,
        "strategy.order_qty",
        &config.strategy.order_qty,
    );

    check_non_empty(&mut errors, "secrets.region", &config.secrets.region);
    check_ssm_path(&mut errors, "secrets.pk", &config.secrets.pk);
    check_ssm_path(&mut errors, "secrets.api_key", &config.secrets.api_key);
    check_ssm_path(
        &mut errors,
        "secrets.api_secret",
        &config.secrets.api_secret,
    );
    check_ssm_path(
        &mut errors,
        "secrets.passphrase",
        &config.secrets.passphrase,
    );

    if config.rulesets.is_empty() {
        let default_reference = LiveReferenceInput::default();
        if !config.reference.publish_topic.trim().is_empty()
            || config.reference.min_publish_interval_ms != default_reference.min_publish_interval_ms
            || !config.reference.venues.is_empty()
        {
            push_error(
                &mut errors,
                "reference",
                "orphaned_phase1_reference",
                "reference must not be configured unless at least one ruleset is enabled"
                    .to_string(),
            );
        }

        if config.audit.is_some() {
            push_error(
                &mut errors,
                "audit",
                "orphaned_phase1_audit",
                "audit must not be configured unless at least one ruleset is enabled".to_string(),
            );
        }
    }

    if !config.rulesets.is_empty() && config.audit.is_none() {
        push_error(
            &mut errors,
            "audit",
            "missing_audit",
            "audit must be configured when rulesets are enabled".to_string(),
        );
    }

    if !config.rulesets.is_empty() {
        check_non_empty(
            &mut errors,
            "reference.publish_topic",
            &config.reference.publish_topic,
        );
        check_positive_u64(
            &mut errors,
            "reference.min_publish_interval_ms",
            config.reference.min_publish_interval_ms,
        );
    }

    let mut ruleset_id_indices: HashMap<&str, usize> = HashMap::new();
    for (i, ruleset) in config.rulesets.iter().enumerate() {
        let field = format!("rulesets[{i}].id");
        check_non_empty(&mut errors, &field, &ruleset.id);

        let tag_slug_field = format!("rulesets[{i}].tag_slug");
        check_non_empty(&mut errors, &tag_slug_field, &ruleset.tag_slug);

        let resolution_basis_field = format!("rulesets[{i}].resolution_basis");
        check_non_empty(
            &mut errors,
            &resolution_basis_field,
            &ruleset.resolution_basis,
        );

        let min_expiry_field = format!("rulesets[{i}].min_time_to_expiry_secs");
        check_positive_u64(
            &mut errors,
            &min_expiry_field,
            ruleset.min_time_to_expiry_secs,
        );

        let max_expiry_field = format!("rulesets[{i}].max_time_to_expiry_secs");
        check_positive_u64(
            &mut errors,
            &max_expiry_field,
            ruleset.max_time_to_expiry_secs,
        );
        if ruleset.max_time_to_expiry_secs < ruleset.min_time_to_expiry_secs {
            push_error(
                &mut errors,
                &max_expiry_field,
                "invalid_max_time_to_expiry_secs",
                format!(
                    "{max_expiry_field} must be >= {min_expiry_field}, got {} < {}",
                    ruleset.max_time_to_expiry_secs, ruleset.min_time_to_expiry_secs
                ),
            );
        }
        let freeze_field = format!("rulesets[{i}].freeze_before_end_secs");
        if ruleset.freeze_before_end_secs < ruleset.min_time_to_expiry_secs {
            push_error(
                &mut errors,
                &freeze_field,
                "invalid_freeze_before_end_secs",
                format!(
                    "{freeze_field} must be >= {min_expiry_field}, got {} < {}",
                    ruleset.freeze_before_end_secs, ruleset.min_time_to_expiry_secs
                ),
            );
        }

        let min_liquidity_field = format!("rulesets[{i}].min_liquidity_num");
        check_non_negative_finite_f64(&mut errors, &min_liquidity_field, ruleset.min_liquidity_num);
        check_positive_u64(
            &mut errors,
            &format!("rulesets[{i}].selector_poll_interval_ms"),
            ruleset.selector_poll_interval_ms,
        );
        check_positive_u64(
            &mut errors,
            &format!("rulesets[{i}].candidate_load_timeout_secs"),
            ruleset.candidate_load_timeout_secs,
        );

        if let Some(first_index) = first_seen_index(&mut ruleset_id_indices, &ruleset.id, i) {
            push_error(
                &mut errors,
                "rulesets",
                "duplicate_ruleset_id",
                format!(
                    "rulesets[{i}] has duplicate id \"{}\" (first defined at rulesets[{first_index}])",
                    ruleset.id
                ),
            );
        }
    }

    for (i, venue) in config.reference.venues.iter().enumerate() {
        let name_field = format!("reference.venues[{i}].name");
        check_non_empty(&mut errors, &name_field, &venue.name);

        let instrument_id_field = format!("reference.venues[{i}].instrument_id");
        check_non_empty(&mut errors, &instrument_id_field, &venue.instrument_id);

        let weight_field = format!("reference.venues[{i}].base_weight");
        check_positive_finite_f64(&mut errors, &weight_field, venue.base_weight);

        let stale_field = format!("reference.venues[{i}].stale_after_ms");
        check_positive_u64(&mut errors, &stale_field, venue.stale_after_ms);

        let disable_field = format!("reference.venues[{i}].disable_after_ms");
        if venue.disable_after_ms < venue.stale_after_ms {
            push_error(
                &mut errors,
                &disable_field,
                "invalid_disable_after_ms",
                format!(
                    "{disable_field} must be >= {stale_field}, got {} < {}",
                    venue.disable_after_ms, venue.stale_after_ms
                ),
            );
        }
    }

    if let Some(audit) = config.audit.as_ref() {
        check_non_empty(&mut errors, "audit.local_dir", &audit.local_dir);
        check_non_empty(&mut errors, "audit.s3_uri", &audit.s3_uri);
        check_positive_u64(
            &mut errors,
            "audit.ship_interval_secs",
            audit.ship_interval_secs,
        );
        check_positive_u64(
            &mut errors,
            "audit.upload_attempt_timeout_secs",
            audit.upload_attempt_timeout_secs,
        );
        check_positive_u64(&mut errors, "audit.roll_max_bytes", audit.roll_max_bytes);
        check_positive_u64(&mut errors, "audit.roll_max_secs", audit.roll_max_secs);
        check_positive_u64(
            &mut errors,
            "audit.max_local_backlog_bytes",
            audit.max_local_backlog_bytes,
        );
    }

    if !config.streaming.catalog_path.trim().is_empty() {
        check_positive_u64(
            &mut errors,
            "streaming.flush_interval_ms",
            config.streaming.flush_interval_ms,
        );
    }
    check_contract_path_catalog_dependency(
        &mut errors,
        &config.streaming.catalog_path,
        config.streaming.contract_path.as_deref(),
    );
    check_live_local_contract_path_shape(&mut errors, config.streaming.contract_path.as_deref());

    errors.sort();
    errors
}

/// Validate a rendered runtime config before it reaches the NT builder.
/// Checks cross-section consistency and re-applies the same domain validation
/// that the local live config uses.
pub fn validate_runtime(config: &Config) -> Vec<ValidationError> {
    let mut errors = Vec::new();

    check_non_empty(&mut errors, "node.name", &config.node.name);
    check_nt_hyphenated(
        &mut errors,
        "node.trader_id",
        &config.node.trader_id,
        split_last_hyphen,
        "NAME-TAG",
    );
    check_allowlist(
        &mut errors,
        "node.environment",
        &config.node.environment,
        VALID_ENVIRONMENTS,
        "invalid_environment",
    );
    check_allowlist(
        &mut errors,
        "logging.stdout_level",
        &config.logging.stdout_level,
        VALID_LOG_LEVELS,
        "invalid_log_level",
    );
    check_allowlist(
        &mut errors,
        "logging.file_level",
        &config.logging.file_level,
        VALID_LOG_LEVELS,
        "invalid_log_level",
    );
    check_positive_u64(
        &mut errors,
        "node.timeout_connection_secs",
        config.node.timeout_connection_secs,
    );
    check_positive_u64(
        &mut errors,
        "node.timeout_reconciliation_secs",
        config.node.timeout_reconciliation_secs,
    );
    check_positive_u64(
        &mut errors,
        "node.timeout_portfolio_secs",
        config.node.timeout_portfolio_secs,
    );
    check_positive_u64(
        &mut errors,
        "node.timeout_disconnection_secs",
        config.node.timeout_disconnection_secs,
    );
    check_positive_u64(
        &mut errors,
        "node.delay_post_stop_secs",
        config.node.delay_post_stop_secs,
    );
    check_positive_u64(
        &mut errors,
        "node.delay_shutdown_secs",
        config.node.delay_shutdown_secs,
    );
    if !config.streaming.catalog_path.trim().is_empty() {
        check_positive_u64(
            &mut errors,
            "streaming.flush_interval_ms",
            config.streaming.flush_interval_ms,
        );
    }
    check_contract_path_catalog_dependency(
        &mut errors,
        &config.streaming.catalog_path,
        config.streaming.contract_path.as_deref(),
    );
    check_runtime_contract_path_shape(&mut errors, config.streaming.contract_path.as_deref());

    let mut data_name_indices: HashMap<&str, usize> = HashMap::new();
    for (i, client) in config.data_clients.iter().enumerate() {
        let name_field = format!("data_clients[{i}].name");
        let type_field = format!("data_clients[{i}].type");
        check_nt_ascii(&mut errors, &name_field, &client.name);
        check_allowlist(
            &mut errors,
            &type_field,
            &client.kind,
            VALID_DATA_CLIENT_TYPES,
            "unsupported_type",
        );
        if let Some(first_index) = first_seen_index(&mut data_name_indices, &client.name, i) {
            push_error(
                &mut errors,
                "data_clients",
                "duplicate_name",
                format!(
                    "data_clients[{i}] has duplicate name \"{}\" (first defined at data_clients[{first_index}])",
                    client.name
                ),
            );
        }

        let event_slugs_field = format!("data_clients[{i}].config.event_slugs");
        if let Some(event_slugs) = get_required_array(
            &mut errors,
            &client.config,
            "event_slugs",
            &event_slugs_field,
            "missing_event_slugs",
        ) {
            for (j, event_slug) in event_slugs.iter().enumerate() {
                let field = format!("data_clients[{i}].config.event_slugs[{j}]");
                match event_slug.as_str() {
                    Some(event_slug) => {
                        check_non_empty_no_whitespace(&mut errors, &field, event_slug)
                    }
                    None => push_error(
                        &mut errors,
                        &field,
                        "invalid_type",
                        "must be a string".to_string(),
                    ),
                }
            }
            if event_slugs.is_empty() {
                push_error(
                    &mut errors,
                    &event_slugs_field,
                    "empty",
                    "must not be empty, got []".to_string(),
                );
            }
        }
    }

    let mut exec_name_indices: HashMap<&str, usize> = HashMap::new();
    for (i, client) in config.exec_clients.iter().enumerate() {
        let name_field = format!("exec_clients[{i}].name");
        let type_field = format!("exec_clients[{i}].type");
        check_nt_ascii(&mut errors, &name_field, &client.name);
        check_allowlist(
            &mut errors,
            &type_field,
            &client.kind,
            VALID_EXEC_CLIENT_TYPES,
            "unsupported_type",
        );
        if let Some(first_index) = first_seen_index(&mut exec_name_indices, &client.name, i) {
            push_error(
                &mut errors,
                "exec_clients",
                "duplicate_name",
                format!(
                    "exec_clients[{i}] has duplicate name \"{}\" (first defined at exec_clients[{first_index}])",
                    client.name
                ),
            );
        }

        let account_id_field = format!("exec_clients[{i}].config.account_id");
        if let Some(account_id) = get_required_str(
            &mut errors,
            &client.config,
            "account_id",
            &account_id_field,
            "missing_account_id",
        ) {
            check_nt_hyphenated(
                &mut errors,
                &account_id_field,
                account_id,
                split_first_hyphen,
                "ISSUER-ACCOUNT",
            );
        }

        let funder_field = format!("exec_clients[{i}].config.funder");
        if let Some(funder) = get_required_str(
            &mut errors,
            &client.config,
            "funder",
            &funder_field,
            "missing_funder",
        ) {
            check_hex_prefixed(&mut errors, &funder_field, funder);
        }

        let signature_type_field = format!("exec_clients[{i}].config.signature_type");
        if let Some(signature_type) = get_required_i64(
            &mut errors,
            &client.config,
            "signature_type",
            &signature_type_field,
            "missing_signature_type",
        ) {
            check_signature_type(&mut errors, &signature_type_field, signature_type);
        }

        let region_field = format!("exec_clients[{i}].secrets.region");
        check_non_empty(&mut errors, &region_field, &client.secrets.region);

        check_required_ssm_path_opt(
            &mut errors,
            &format!("exec_clients[{i}].secrets.pk"),
            client.secrets.pk.as_ref(),
            "missing_pk",
        );
        check_required_ssm_path_opt(
            &mut errors,
            &format!("exec_clients[{i}].secrets.api_key"),
            client.secrets.api_key.as_ref(),
            "missing_api_key",
        );
        check_required_ssm_path_opt(
            &mut errors,
            &format!("exec_clients[{i}].secrets.api_secret"),
            client.secrets.api_secret.as_ref(),
            "missing_api_secret",
        );
        check_required_ssm_path_opt(
            &mut errors,
            &format!("exec_clients[{i}].secrets.passphrase"),
            client.secrets.passphrase.as_ref(),
            "missing_passphrase",
        );
    }

    let mut strategy_id_indices: HashMap<&str, usize> = HashMap::new();
    for (i, strategy) in config.strategies.iter().enumerate() {
        let strategy_type_field = format!("strategies[{i}].type");
        check_allowlist(
            &mut errors,
            &strategy_type_field,
            &strategy.kind,
            VALID_STRATEGY_TYPES,
            "unsupported_type",
        );
        match strategy.config.get("client_id") {
            None => push_error(
                &mut errors,
                &format!("strategies[{i}].config.client_id"),
                "missing_client_id",
                "is missing required string field".to_string(),
            ),
            Some(value) => {
                let field = format!("strategies[{i}].config.client_id");
                if let Some(client_id) = value.as_str() {
                    check_nt_ascii(&mut errors, &field, client_id);
                    if !exec_name_indices.contains_key(client_id) {
                        push_error(
                            &mut errors,
                            "strategies",
                            "unknown_client_id",
                            format!(
                                "strategies[{i}] references client_id \"{client_id}\" which does not match any exec_client name"
                            ),
                        );
                    }
                } else {
                    push_error(
                        &mut errors,
                        &field,
                        "wrong_type",
                        format!("must be a string, got {} value", value.type_str()),
                    );
                }
            }
        }

        match strategy.config.get("strategy_id") {
            None => push_error(
                &mut errors,
                &format!("strategies[{i}].config.strategy_id"),
                "missing_strategy_id",
                "is missing required string field".to_string(),
            ),
            Some(value) => {
                let field = format!("strategies[{i}].config.strategy_id");
                if let Some(strategy_id) = value.as_str() {
                    if let Some(first_index) =
                        first_seen_index(&mut strategy_id_indices, strategy_id, i)
                    {
                        push_error(
                            &mut errors,
                            "strategies",
                            "duplicate_strategy_id",
                            format!(
                                "strategies[{i}] has duplicate strategy_id \"{strategy_id}\" (first defined at strategies[{first_index}])"
                            ),
                        );
                    }

                    if strategy_id != "EXTERNAL" {
                        check_nt_hyphenated(
                            &mut errors,
                            &field,
                            strategy_id,
                            split_last_hyphen,
                            "NAME-TAG",
                        );
                    }
                } else {
                    push_error(
                        &mut errors,
                        &field,
                        "wrong_type",
                        format!("must be a string, got {} value", value.type_str()),
                    );
                }
            }
        }

        match strategy.config.get("instrument_id") {
            None => push_error(
                &mut errors,
                &format!("strategies[{i}].config.instrument_id"),
                "missing_instrument_id",
                "is missing required string field".to_string(),
            ),
            Some(value) => {
                let field = format!("strategies[{i}].config.instrument_id");
                if let Some(instrument_id) = value.as_str() {
                    check_instrument_id(&mut errors, &field, instrument_id);
                } else {
                    push_error(
                        &mut errors,
                        &field,
                        "wrong_type",
                        format!("must be a string, got {} value", value.type_str()),
                    );
                }
            }
        }

        match strategy.config.get("order_qty") {
            None => push_error(
                &mut errors,
                &format!("strategies[{i}].config.order_qty"),
                "missing_order_qty",
                "is missing required string field".to_string(),
            ),
            Some(value) => {
                let field = format!("strategies[{i}].config.order_qty");
                if let Some(order_qty) = value.as_str() {
                    check_strictly_positive_qty(&mut errors, &field, order_qty);
                } else {
                    push_error(
                        &mut errors,
                        &field,
                        "wrong_type",
                        format!("must be a string, got {} value", value.type_str()),
                    );
                }
            }
        }
    }

    if config.rulesets.len() > 1 {
        push_error(
            &mut errors,
            "rulesets",
            "phase1_single_active_ruleset",
            format!(
                "Phase 1 supports exactly one active ruleset, got {}",
                config.rulesets.len()
            ),
        );
    }

    if config.rulesets.len() == 1 && config.reference.venues.is_empty() {
        push_error(
            &mut errors,
            "reference.venues",
            "missing_reference_venues",
            "reference.venues must not be empty when a ruleset is configured".to_string(),
        );
    }

    if config.rulesets.is_empty() {
        let default_reference = ReferenceConfig::default();
        if !config.reference.publish_topic.trim().is_empty()
            || config.reference.min_publish_interval_ms != default_reference.min_publish_interval_ms
            || !config.reference.venues.is_empty()
        {
            push_error(
                &mut errors,
                "reference",
                "orphaned_phase1_reference",
                "reference must not be configured unless at least one ruleset is enabled"
                    .to_string(),
            );
        }

        if config.audit.is_some() {
            push_error(
                &mut errors,
                "audit",
                "orphaned_phase1_audit",
                "audit must not be configured unless at least one ruleset is enabled".to_string(),
            );
        }
    }

    if !config.rulesets.is_empty() && config.audit.is_none() {
        push_error(
            &mut errors,
            "audit",
            "missing_audit",
            "audit must be configured when rulesets are enabled".to_string(),
        );
    }

    if !config.rulesets.is_empty() {
        check_non_empty(
            &mut errors,
            "reference.publish_topic",
            &config.reference.publish_topic,
        );
        check_positive_u64(
            &mut errors,
            "reference.min_publish_interval_ms",
            config.reference.min_publish_interval_ms,
        );
    }

    let mut reference_name_indices: HashMap<&str, usize> = HashMap::new();
    for (i, venue) in config.reference.venues.iter().enumerate() {
        let name_field = format!("reference.venues[{i}].name");
        check_non_empty(&mut errors, &name_field, &venue.name);

        let instrument_id_field = format!("reference.venues[{i}].instrument_id");
        check_non_empty(&mut errors, &instrument_id_field, &venue.instrument_id);

        let weight_field = format!("reference.venues[{i}].base_weight");
        check_positive_finite_f64(&mut errors, &weight_field, venue.base_weight);

        let stale_field = format!("reference.venues[{i}].stale_after_ms");
        check_positive_u64(&mut errors, &stale_field, venue.stale_after_ms);

        let disable_field = format!("reference.venues[{i}].disable_after_ms");
        if venue.disable_after_ms < venue.stale_after_ms {
            push_error(
                &mut errors,
                &disable_field,
                "invalid_disable_after_ms",
                format!(
                    "{disable_field} must be >= {stale_field}, got {} < {}",
                    venue.disable_after_ms, venue.stale_after_ms
                ),
            );
        }

        if let Some(first_index) = first_seen_index(&mut reference_name_indices, &venue.name, i) {
            push_error(
                &mut errors,
                "reference.venues",
                "duplicate_name",
                format!(
                    "reference.venues[{i}] has duplicate name \"{}\" (first defined at reference.venues[{first_index}])",
                    venue.name
                ),
            );
        }
    }

    if let Some(audit) = config.audit.as_ref() {
        check_non_empty(&mut errors, "audit.local_dir", &audit.local_dir);
        check_non_empty(&mut errors, "audit.s3_uri", &audit.s3_uri);
        check_positive_u64(
            &mut errors,
            "audit.ship_interval_secs",
            audit.ship_interval_secs,
        );
        check_positive_u64(
            &mut errors,
            "audit.upload_attempt_timeout_secs",
            audit.upload_attempt_timeout_secs,
        );
        check_positive_u64(&mut errors, "audit.roll_max_bytes", audit.roll_max_bytes);
        check_positive_u64(&mut errors, "audit.roll_max_secs", audit.roll_max_secs);
        check_positive_u64(
            &mut errors,
            "audit.max_local_backlog_bytes",
            audit.max_local_backlog_bytes,
        );
    }

    let has_polymarket_reference = config
        .reference
        .venues
        .iter()
        .any(|venue| venue.kind == ReferenceVenueKind::Polymarket);
    if has_polymarket_reference {
        let polymarket_data_clients = config
            .data_clients
            .iter()
            .filter(|client| client.kind == "polymarket")
            .count();

        if polymarket_data_clients == 0 {
            push_error(
                &mut errors,
                "reference.venues",
                "missing_primary_polymarket_client",
                "reference venue kind polymarket requires the primary polymarket data client to already be configured".to_string(),
            );
        } else if polymarket_data_clients > 1 {
            push_error(
                &mut errors,
                "data_clients",
                "duplicate_polymarket_client_for_reference",
                "reference venue kind polymarket must reuse the primary polymarket data client instead of registering a second polymarket client".to_string(),
            );
        }
    }

    let mut ruleset_id_indices: HashMap<&str, usize> = HashMap::new();
    for (i, ruleset) in config.rulesets.iter().enumerate() {
        let id_field = format!("rulesets[{i}].id");
        check_non_empty(&mut errors, &id_field, &ruleset.id);

        let tag_slug_field = format!("rulesets[{i}].tag_slug");
        check_non_empty(&mut errors, &tag_slug_field, &ruleset.tag_slug);

        let resolution_basis_field = format!("rulesets[{i}].resolution_basis");
        check_non_empty(
            &mut errors,
            &resolution_basis_field,
            &ruleset.resolution_basis,
        );

        let min_expiry_field = format!("rulesets[{i}].min_time_to_expiry_secs");
        check_positive_u64(
            &mut errors,
            &min_expiry_field,
            ruleset.min_time_to_expiry_secs,
        );

        let max_expiry_field = format!("rulesets[{i}].max_time_to_expiry_secs");
        check_positive_u64(
            &mut errors,
            &max_expiry_field,
            ruleset.max_time_to_expiry_secs,
        );
        if ruleset.max_time_to_expiry_secs < ruleset.min_time_to_expiry_secs {
            push_error(
                &mut errors,
                &max_expiry_field,
                "invalid_max_time_to_expiry_secs",
                format!(
                    "{max_expiry_field} must be >= {min_expiry_field}, got {} < {}",
                    ruleset.max_time_to_expiry_secs, ruleset.min_time_to_expiry_secs
                ),
            );
        }
        let freeze_field = format!("rulesets[{i}].freeze_before_end_secs");
        if ruleset.freeze_before_end_secs < ruleset.min_time_to_expiry_secs {
            push_error(
                &mut errors,
                &freeze_field,
                "invalid_freeze_before_end_secs",
                format!(
                    "{freeze_field} must be >= {min_expiry_field}, got {} < {}",
                    ruleset.freeze_before_end_secs, ruleset.min_time_to_expiry_secs
                ),
            );
        }

        let min_liquidity_field = format!("rulesets[{i}].min_liquidity_num");
        check_non_negative_finite_f64(&mut errors, &min_liquidity_field, ruleset.min_liquidity_num);
        check_positive_u64(
            &mut errors,
            &format!("rulesets[{i}].selector_poll_interval_ms"),
            ruleset.selector_poll_interval_ms,
        );
        check_positive_u64(
            &mut errors,
            &format!("rulesets[{i}].candidate_load_timeout_secs"),
            ruleset.candidate_load_timeout_secs,
        );

        if let Some(first_index) = first_seen_index(&mut ruleset_id_indices, &ruleset.id, i) {
            push_error(
                &mut errors,
                "rulesets",
                "duplicate_ruleset_id",
                format!(
                    "rulesets[{i}] has duplicate id \"{}\" (first defined at rulesets[{first_index}])",
                    ruleset.id
                ),
            );
        }

        if let Some(required_kind) = implied_reference_venue_kind(&ruleset.resolution_basis) {
            let has_matching_kind = config
                .reference
                .venues
                .iter()
                .any(|venue| venue.kind == required_kind);

            if !has_matching_kind {
                push_error(
                    &mut errors,
                    &format!("rulesets[{i}].resolution_basis"),
                    "missing_reference_venue_family",
                    format!(
                        "rulesets[{i}].resolution_basis requires a configured reference venue of kind {:?}",
                        required_kind
                    ),
                );
            }
        }
    }

    errors.sort();
    errors
}

#[cfg(test)]
#[path = "validate_tests.rs"]
mod tests;
