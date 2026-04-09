use crate::config::Config;
use crate::live_config::LiveLocalConfig;
use rust_decimal::Decimal;
use std::collections::HashSet;
use std::path::Path;
use std::str::FromStr;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ValidationError {
    pub field: &'static str,
    pub code: &'static str,
    pub message: String,
}

impl std::fmt::Display for ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.field, self.message)
    }
}

// ═══════════════════════════════════════════════════════════════════
// NT-contract-derived helpers
// ═══════════════════════════════════════════════════════════════════

/// Mirrors NT's `check_valid_string_ascii`: non-empty, non-whitespace-only, ASCII-only.
fn check_nt_ascii(errors: &mut Vec<ValidationError>, field: &'static str, value: &str) {
    if value.is_empty() {
        errors.push(ValidationError {
            field,
            code: "empty",
            message: format!("{field} must not be empty, got \"\""),
        });
    } else if value.trim().is_empty() {
        errors.push(ValidationError {
            field,
            code: "whitespace_only",
            message: format!("{field} must not be whitespace-only, got \"{value}\""),
        });
    } else if !value.is_ascii() {
        errors.push(ValidationError {
            field,
            code: "non_ascii",
            message: format!("{field} must be ASCII-only, got \"{value}\""),
        });
    }
}

/// Shared hyphen-check logic. `split_fn` selects the split direction to
/// match the exact NT constructor for each identifier type.
fn check_nt_hyphenated(
    errors: &mut Vec<ValidationError>,
    field: &'static str,
    value: &str,
    split_fn: fn(&str) -> Option<(&str, &str)>,
) {
    // Run the base ASCII checks first.  If any fail, don't pile on with
    // hyphen checks (the value is already structurally invalid).
    if value.is_empty() || value.trim().is_empty() || !value.is_ascii() {
        check_nt_ascii(errors, field, value);
        return;
    }

    match split_fn(value) {
        None => {
            errors.push(ValidationError {
                field,
                code: "missing_hyphen",
                message: format!(
                    "{field} must contain a hyphen separating name and tag, got \"{value}\""
                ),
            });
        }
        Some(("", _)) => {
            errors.push(ValidationError {
                field,
                code: "empty_name_part",
                message: format!("{field} has empty name part before hyphen, got \"{value}\""),
            });
        }
        Some((_, "")) => {
            errors.push(ValidationError {
                field,
                code: "empty_tag_part",
                message: format!("{field} has empty tag part after hyphen, got \"{value}\""),
            });
        }
        Some(_) => {} // valid
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
fn check_non_empty_no_whitespace(
    errors: &mut Vec<ValidationError>,
    field: &'static str,
    value: &str,
) {
    if value.is_empty() {
        errors.push(ValidationError {
            field,
            code: "empty",
            message: format!("{field} must not be empty, got \"\""),
        });
    } else if value.trim().is_empty() {
        errors.push(ValidationError {
            field,
            code: "whitespace_only",
            message: format!("{field} must not be whitespace-only, got \"{value}\""),
        });
    } else if value.contains(char::is_whitespace) {
        errors.push(ValidationError {
            field,
            code: "contains_whitespace",
            message: format!("{field} must not contain whitespace, got \"{value}\""),
        });
    }
}

/// NT `InstrumentId::from` uses `rsplit_once('.')` -> (symbol, venue).
/// We enforce venue == "POLYMARKET" and symbol non-empty.
fn check_instrument_id(errors: &mut Vec<ValidationError>, value: &str) {
    let field = "polymarket.instrument_id";
    if value.is_empty() {
        errors.push(ValidationError {
            field,
            code: "empty",
            message: format!("{field} must not be empty, got \"\""),
        });
    } else if !value.ends_with(".POLYMARKET") {
        errors.push(ValidationError {
            field,
            code: "missing_venue_suffix",
            message: format!("{field} must end with .POLYMARKET, got \"{value}\""),
        });
    } else {
        // value ends with ".POLYMARKET" -- check symbol part is non-empty.
        let symbol = &value[..value.len() - ".POLYMARKET".len()];
        if symbol.is_empty() {
            errors.push(ValidationError {
                field,
                code: "empty_symbol",
                message: format!(
                    "{field} symbol part before .POLYMARKET must not be empty, got \"{value}\""
                ),
            });
        }
    }
}

fn check_hex_prefixed(errors: &mut Vec<ValidationError>, field: &'static str, value: &str) {
    if value.is_empty() {
        errors.push(ValidationError {
            field,
            code: "empty",
            message: format!("{field} must not be empty, got \"\""),
        });
    } else if !value.starts_with("0x") {
        errors.push(ValidationError {
            field,
            code: "missing_hex_prefix",
            message: format!("{field} must start with 0x, got \"{value}\""),
        });
    }
}

/// Mirrors NT's `Quantity::from_str` exactly: strip underscores, parse via
/// `rust_decimal::Decimal` (same crate NT uses), check non-negative, check
/// `decimal.scale() <= 9` (FIXED_PRECISION without high-precision feature).
fn check_positive_qty(errors: &mut Vec<ValidationError>, value: &str) {
    let field = "strategy.order_qty";
    let clean = value.replace('_', "");

    let decimal = if clean.contains('e') || clean.contains('E') {
        Decimal::from_scientific(&clean)
    } else {
        Decimal::from_str(&clean)
    };

    match decimal {
        Ok(d) if d.is_sign_negative() || d.is_zero() => {
            errors.push(ValidationError {
                field,
                code: "not_positive_number",
                message: format!("{field} must be a positive number, got \"{value}\""),
            });
        }
        Ok(d) => {
            let precision = d.scale();
            if precision > 9 {
                errors.push(ValidationError {
                    field,
                    code: "excessive_precision",
                    message: format!(
                        "{field} precision must be <= 9 decimal digits, got \"{value}\""
                    ),
                });
            }
        }
        Err(_) => {
            errors.push(ValidationError {
                field,
                code: "not_positive_number",
                message: format!("{field} must be a positive number, got \"{value}\""),
            });
        }
    }
}

fn check_ssm_path(errors: &mut Vec<ValidationError>, field: &'static str, value: &str) {
    if value.is_empty() {
        errors.push(ValidationError {
            field,
            code: "empty",
            message: format!("{field} must not be empty, got \"\""),
        });
    } else if !value.starts_with('/') {
        errors.push(ValidationError {
            field,
            code: "missing_leading_slash",
            message: format!(
                "{field} must be an absolute SSM path starting with /, got \"{value}\""
            ),
        });
    }
}

/// Allowlist check for a field that must be one of an exact set of values.
fn check_allowlist(
    errors: &mut Vec<ValidationError>,
    field: &'static str,
    value: &str,
    allowed: &[&str],
    code: &'static str,
) {
    if !allowed.contains(&value) {
        errors.push(ValidationError {
            field,
            code,
            message: format!("{field} must be one of {allowed:?}, got \"{value}\""),
        });
    }
}

// ═══════════════════════════════════════════════════════════════════
// Public validators
// ═══════════════════════════════════════════════════════════════════

const VALID_ENVIRONMENTS: &[&str] = &["Live", "Sandbox"];
const VALID_LOG_LEVELS: &[&str] = &["Trace", "Debug", "Info", "Warn", "Error", "Off"];

/// Validate a human-edited live local config before rendering.
/// Returns all validation errors found, sorted by field path for deterministic output.
pub fn validate_live_local(config: &LiveLocalConfig) -> Vec<ValidationError> {
    let mut errors = Vec::new();

    // -- node --------------------------------------------------------
    check_nt_ascii(&mut errors, "node.name", &config.node.name);
    // NT TraderId uses rsplit_once('-')
    check_nt_hyphenated(
        &mut errors,
        "node.trader_id",
        &config.node.trader_id,
        split_last_hyphen,
    );
    check_allowlist(
        &mut errors,
        "node.environment",
        &config.node.environment,
        VALID_ENVIRONMENTS,
        "invalid_environment",
    );

    // -- logging -----------------------------------------------------
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

    // -- polymarket ---------------------------------------------------
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
    check_instrument_id(&mut errors, &config.polymarket.instrument_id);
    // NT AccountId uses split_once('-')
    check_nt_hyphenated(
        &mut errors,
        "polymarket.account_id",
        &config.polymarket.account_id,
        split_first_hyphen,
    );
    check_hex_prefixed(&mut errors, "polymarket.funder", &config.polymarket.funder);

    // -- strategy ----------------------------------------------------
    // StrategyId: must be NAME-TAG *or* literal "EXTERNAL"
    if config.strategy.strategy_id != "EXTERNAL" {
        // NT StrategyId uses rsplit_once('-')
        check_nt_hyphenated(
            &mut errors,
            "strategy.strategy_id",
            &config.strategy.strategy_id,
            split_last_hyphen,
        );
    }
    check_positive_qty(&mut errors, &config.strategy.order_qty);

    // -- secrets (SSM paths) -----------------------------------------
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

    if let Some(contract_path) = config.streaming.contract_path.as_deref()
        && !contract_path.trim().is_empty()
        && config.streaming.catalog_path.trim().is_empty()
    {
        errors.push(ValidationError {
            field: "streaming.contract_path",
            code: "requires_catalog_path",
            message: "streaming.contract_path requires non-empty streaming.catalog_path".to_string(),
        });
    }

    errors.sort();
    errors
}

/// Validate a rendered runtime config before it reaches the NT builder.
/// Checks cross-section consistency that only exists at the runtime layer
/// (multiple clients, strategies referencing clients by name).
pub fn validate_runtime(config: &Config) -> Vec<ValidationError> {
    let mut errors = Vec::new();

    // -- Duplicate client names --------------------------------------
    let mut data_names: HashSet<&str> = HashSet::new();
    for client in &config.data_clients {
        if !data_names.insert(&client.name) {
            errors.push(ValidationError {
                field: "data_clients",
                code: "duplicate_name",
                message: format!("duplicate data client name: \"{}\"", client.name),
            });
        }
    }

    let mut exec_names: HashSet<&str> = HashSet::new();
    for client in &config.exec_clients {
        if !exec_names.insert(&client.name) {
            errors.push(ValidationError {
                field: "exec_clients",
                code: "duplicate_name",
                message: format!("duplicate exec client name: \"{}\"", client.name),
            });
        }
    }

    // -- Duplicate strategy IDs --------------------------------------
    let mut strategy_ids: HashSet<&str> = HashSet::new();
    for (i, strategy) in config.strategies.iter().enumerate() {
        if let Some(sid) = strategy.config.get("strategy_id").and_then(|v| v.as_str())
            && !strategy_ids.insert(sid)
        {
            errors.push(ValidationError {
                field: "strategies",
                code: "duplicate_strategy_id",
                message: format!("strategies[{i}] has duplicate strategy_id \"{sid}\""),
            });
        }
    }

    // -- Strategy required fields and client_id references -----------
    for (i, strategy) in config.strategies.iter().enumerate() {
        match strategy.config.get("client_id").and_then(|v| v.as_str()) {
            None => {
                errors.push(ValidationError {
                    field: "strategies",
                    code: "missing_client_id",
                    message: format!("strategies[{i}] is missing required client_id field"),
                });
            }
            Some(client_id) if !exec_names.contains(client_id) => {
                errors.push(ValidationError {
                    field: "strategies",
                    code: "unknown_client_id",
                    message: format!(
                        "strategies[{i}] references client_id \"{client_id}\" \
                         which does not match any exec_client name"
                    ),
                });
            }
            Some(_) => {}
        }

        if strategy
            .config
            .get("strategy_id")
            .and_then(|v| v.as_str())
            .is_none()
        {
            errors.push(ValidationError {
                field: "strategies",
                code: "missing_strategy_id",
                message: format!("strategies[{i}] is missing required strategy_id field"),
            });
        }

        if strategy
            .config
            .get("instrument_id")
            .and_then(|v| v.as_str())
            .is_none()
        {
            errors.push(ValidationError {
                field: "strategies",
                code: "missing_instrument_id",
                message: format!("strategies[{i}] is missing required instrument_id field"),
            });
        }

        if strategy
            .config
            .get("order_qty")
            .and_then(|v| v.as_str())
            .is_none()
        {
            errors.push(ValidationError {
                field: "strategies",
                code: "missing_order_qty",
                message: format!("strategies[{i}] is missing required order_qty field"),
            });
        }
    }

    if let Some(contract_path) = config.streaming.contract_path.as_deref() {
        if contract_path.trim().is_empty() {
            errors.push(ValidationError {
                field: "streaming.contract_path",
                code: "empty",
                message: "streaming.contract_path must not be empty when provided".to_string(),
            });
        } else if contract_path.contains("://") {
            errors.push(ValidationError {
                field: "streaming.contract_path",
                code: "non_local",
                message: format!(
                    "streaming.contract_path must be a local absolute path, got \"{contract_path}\""
                ),
            });
        } else if !Path::new(contract_path).is_absolute() {
            errors.push(ValidationError {
                field: "streaming.contract_path",
                code: "not_absolute",
                message: format!(
                    "streaming.contract_path must be absolute at runtime, got \"{contract_path}\""
                ),
            });
        }
    }

    errors.sort();
    errors
}

#[cfg(test)]
#[path = "validate_tests.rs"]
mod tests;
