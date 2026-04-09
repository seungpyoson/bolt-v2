use crate::config::Config;
use crate::live_config::LiveLocalConfig;
use std::collections::HashSet;

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

/// Validate a human-edited live local config before rendering.
/// Returns all validation errors found, sorted by field path for deterministic output.
pub fn validate_live_local(config: &LiveLocalConfig) -> Vec<ValidationError> {
    let mut errors = Vec::new();

    // ── node ─────────────────────────────────────────────────
    check_non_empty(&mut errors, "node.name", &config.node.name);
    check_non_empty(&mut errors, "node.trader_id", &config.node.trader_id);

    // ── polymarket ───────────────────────────────────────────
    check_non_empty_no_whitespace(
        &mut errors,
        "polymarket.event_slug",
        &config.polymarket.event_slug,
    );
    check_instrument_id(&mut errors, &config.polymarket.instrument_id);
    check_non_empty(&mut errors, "polymarket.account_id", &config.polymarket.account_id);
    check_hex_prefixed(&mut errors, "polymarket.funder", &config.polymarket.funder);

    // ── strategy ─────────────────────────────────────────────
    check_non_empty(&mut errors, "strategy.strategy_id", &config.strategy.strategy_id);
    check_positive_qty(&mut errors, &config.strategy.order_qty);

    // ── secrets (SSM paths) ──────────────────────────────────
    check_ssm_path(&mut errors, "secrets.pk", &config.secrets.pk);
    check_ssm_path(&mut errors, "secrets.api_key", &config.secrets.api_key);
    check_ssm_path(&mut errors, "secrets.api_secret", &config.secrets.api_secret);
    check_ssm_path(&mut errors, "secrets.passphrase", &config.secrets.passphrase);

    errors.sort();
    errors
}

fn check_non_empty(errors: &mut Vec<ValidationError>, field: &'static str, value: &str) {
    if value.is_empty() {
        errors.push(ValidationError {
            field,
            code: "empty",
            message: format!("{field} must not be empty"),
        });
    }
}

fn check_non_empty_no_whitespace(
    errors: &mut Vec<ValidationError>,
    field: &'static str,
    value: &str,
) {
    if value.is_empty() {
        errors.push(ValidationError {
            field,
            code: "empty",
            message: format!("{field} must not be empty"),
        });
    } else if value.trim().is_empty() {
        errors.push(ValidationError {
            field,
            code: "whitespace_only",
            message: format!("{field} must not be whitespace-only"),
        });
    } else if value.contains(char::is_whitespace) {
        errors.push(ValidationError {
            field,
            code: "contains_whitespace",
            message: format!("{field} must not contain whitespace"),
        });
    }
}

fn check_instrument_id(errors: &mut Vec<ValidationError>, value: &str) {
    let field = "polymarket.instrument_id";
    if value.is_empty() {
        errors.push(ValidationError {
            field,
            code: "empty",
            message: format!("{field} must not be empty"),
        });
    } else if !value.ends_with(".POLYMARKET") {
        errors.push(ValidationError {
            field,
            code: "missing_venue_suffix",
            message: format!("{field} must end with .POLYMARKET"),
        });
    }
}

fn check_hex_prefixed(errors: &mut Vec<ValidationError>, field: &'static str, value: &str) {
    if value.is_empty() {
        errors.push(ValidationError {
            field,
            code: "empty",
            message: format!("{field} must not be empty"),
        });
    } else if !value.starts_with("0x") {
        errors.push(ValidationError {
            field,
            code: "missing_hex_prefix",
            message: format!("{field} must start with 0x"),
        });
    }
}

fn check_positive_qty(errors: &mut Vec<ValidationError>, value: &str) {
    let field = "strategy.order_qty";
    match value.parse::<f64>() {
        Ok(v) if v.is_finite() && v > 0.0 => {}
        _ => {
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
            message: format!("{field} must not be empty"),
        });
    } else if !value.starts_with('/') {
        errors.push(ValidationError {
            field,
            code: "missing_leading_slash",
            message: format!("{field} must be an absolute SSM path starting with /"),
        });
    }
}

/// Validate a rendered runtime config before it reaches the NT builder.
/// Checks cross-section consistency that only exists at the runtime layer
/// (multiple clients, strategies referencing clients by name).
pub fn validate_runtime(config: &Config) -> Vec<ValidationError> {
    let mut errors = Vec::new();

    // ── Duplicate client names ───────────────────────────────
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

    // ── Strategy client_id references an existing exec client ─
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
    }

    errors.sort();
    errors
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal valid config — all fields satisfy validation rules.
    /// Each test overrides one field to trigger a specific error.
    fn valid_toml() -> String {
        r#"
[node]
name = "BOLT-V2-001"
trader_id = "BOLT-001"

[polymarket]
event_slug = "btc-updown-5m"
instrument_id = "0xabc-12345.POLYMARKET"
account_id = "POLYMARKET-001"
funder = "0xabc"

[strategy]
strategy_id = "EXEC_TESTER-001"
order_qty = "5"

[secrets]
pk = "/bolt/poly/pk"
api_key = "/bolt/poly/key"
api_secret = "/bolt/poly/secret"
passphrase = "/bolt/poly/passphrase"
"#
        .to_string()
    }

    fn replace(base: &str, old: &str, new: &str) -> String {
        assert!(
            base.contains(old),
            "test helper: '{old}' not found in base config"
        );
        base.replacen(old, new, 1)
    }

    fn parse(toml: &str) -> LiveLocalConfig {
        toml::from_str(toml).expect("test config should parse")
    }

    fn errors_for(toml: &str) -> Vec<ValidationError> {
        let config = parse(toml);
        let mut errors = validate_live_local(&config);
        errors.sort();
        errors
    }

    fn assert_has_error(errors: &[ValidationError], field: &str, code: &str) {
        assert!(
            errors.iter().any(|e| e.field == field && e.code == code),
            "expected error field={field} code={code}, got: {errors:?}"
        );
    }

    fn assert_no_errors(errors: &[ValidationError]) {
        assert!(errors.is_empty(), "expected no errors, got: {errors:?}");
    }

    // ── Golden path ──────────────────────────────────────────────

    #[test]
    fn valid_config_passes_all_validation() {
        let errors = errors_for(&valid_toml());
        assert_no_errors(&errors);
    }

    // ── node.name ────────────────────────────────────────────────

    #[test]
    fn empty_node_name_rejected() {
        let toml = replace(&valid_toml(), "name = \"BOLT-V2-001\"", "name = \"\"");
        let errors = errors_for(&toml);
        assert_has_error(&errors, "node.name", "empty");
    }

    // ── node.trader_id ───────────────────────────────────────────

    #[test]
    fn empty_trader_id_rejected() {
        let toml = replace(&valid_toml(), "trader_id = \"BOLT-001\"", "trader_id = \"\"");
        let errors = errors_for(&toml);
        assert_has_error(&errors, "node.trader_id", "empty");
    }

    // ── polymarket.event_slug ────────────────────────────────────

    #[test]
    fn empty_event_slug_rejected() {
        let toml = replace(
            &valid_toml(),
            "event_slug = \"btc-updown-5m\"",
            "event_slug = \"\"",
        );
        let errors = errors_for(&toml);
        assert_has_error(&errors, "polymarket.event_slug", "empty");
    }

    #[test]
    fn whitespace_event_slug_rejected() {
        let toml = replace(
            &valid_toml(),
            "event_slug = \"btc-updown-5m\"",
            "event_slug = \"  \"",
        );
        let errors = errors_for(&toml);
        assert_has_error(&errors, "polymarket.event_slug", "whitespace_only");
    }

    // ── polymarket.instrument_id ─────────────────────────────────

    #[test]
    fn empty_instrument_id_rejected() {
        let toml = replace(
            &valid_toml(),
            "instrument_id = \"0xabc-12345.POLYMARKET\"",
            "instrument_id = \"\"",
        );
        let errors = errors_for(&toml);
        assert_has_error(&errors, "polymarket.instrument_id", "empty");
    }

    #[test]
    fn instrument_id_missing_venue_suffix_rejected() {
        let toml = replace(
            &valid_toml(),
            "instrument_id = \"0xabc-12345.POLYMARKET\"",
            "instrument_id = \"0xabc-12345\"",
        );
        let errors = errors_for(&toml);
        assert_has_error(
            &errors,
            "polymarket.instrument_id",
            "missing_venue_suffix",
        );
    }

    #[test]
    fn instrument_id_wrong_venue_suffix_rejected() {
        let toml = replace(
            &valid_toml(),
            "instrument_id = \"0xabc-12345.POLYMARKET\"",
            "instrument_id = \"0xabc-12345.BINANCE\"",
        );
        let errors = errors_for(&toml);
        assert_has_error(
            &errors,
            "polymarket.instrument_id",
            "missing_venue_suffix",
        );
    }

    // ── polymarket.account_id ────────────────────────────────────

    #[test]
    fn empty_account_id_rejected() {
        let toml = replace(
            &valid_toml(),
            "account_id = \"POLYMARKET-001\"",
            "account_id = \"\"",
        );
        let errors = errors_for(&toml);
        assert_has_error(&errors, "polymarket.account_id", "empty");
    }

    // ── polymarket.funder ────────────────────────────────────────

    #[test]
    fn empty_funder_rejected() {
        let toml = replace(&valid_toml(), "funder = \"0xabc\"", "funder = \"\"");
        let errors = errors_for(&toml);
        assert_has_error(&errors, "polymarket.funder", "empty");
    }

    #[test]
    fn funder_missing_hex_prefix_rejected() {
        let toml = replace(&valid_toml(), "funder = \"0xabc\"", "funder = \"abc\"");
        let errors = errors_for(&toml);
        assert_has_error(&errors, "polymarket.funder", "missing_hex_prefix");
    }

    // ── strategy.strategy_id ─────────────────────────────────────

    #[test]
    fn empty_strategy_id_rejected() {
        let toml = replace(
            &valid_toml(),
            "strategy_id = \"EXEC_TESTER-001\"",
            "strategy_id = \"\"",
        );
        let errors = errors_for(&toml);
        assert_has_error(&errors, "strategy.strategy_id", "empty");
    }

    // ── strategy.order_qty ───────────────────────────────────────

    #[test]
    fn order_qty_non_numeric_rejected() {
        let toml = replace(&valid_toml(), "order_qty = \"5\"", "order_qty = \"abc\"");
        let errors = errors_for(&toml);
        assert_has_error(&errors, "strategy.order_qty", "not_positive_number");
    }

    #[test]
    fn order_qty_zero_rejected() {
        let toml = replace(&valid_toml(), "order_qty = \"5\"", "order_qty = \"0\"");
        let errors = errors_for(&toml);
        assert_has_error(&errors, "strategy.order_qty", "not_positive_number");
    }

    #[test]
    fn order_qty_negative_rejected() {
        let toml = replace(&valid_toml(), "order_qty = \"5\"", "order_qty = \"-1\"");
        let errors = errors_for(&toml);
        assert_has_error(&errors, "strategy.order_qty", "not_positive_number");
    }

    // ── secrets SSM paths ────────────────────────────────────────

    #[test]
    fn ssm_path_missing_leading_slash_rejected() {
        let toml = replace(
            &valid_toml(),
            "pk = \"/bolt/poly/pk\"",
            "pk = \"bolt/poly/pk\"",
        );
        let errors = errors_for(&toml);
        assert_has_error(&errors, "secrets.pk", "missing_leading_slash");
    }

    #[test]
    fn all_ssm_paths_validated() {
        let mut toml = valid_toml();
        toml = replace(&toml, "pk = \"/bolt/poly/pk\"", "pk = \"bolt/poly/pk\"");
        toml = replace(
            &toml,
            "api_key = \"/bolt/poly/key\"",
            "api_key = \"bolt/poly/key\"",
        );
        toml = replace(
            &toml,
            "api_secret = \"/bolt/poly/secret\"",
            "api_secret = \"bolt/poly/secret\"",
        );
        toml = replace(
            &toml,
            "passphrase = \"/bolt/poly/passphrase\"",
            "passphrase = \"bolt/poly/passphrase\"",
        );

        let errors = errors_for(&toml);
        assert_has_error(&errors, "secrets.pk", "missing_leading_slash");
        assert_has_error(&errors, "secrets.api_key", "missing_leading_slash");
        assert_has_error(&errors, "secrets.api_secret", "missing_leading_slash");
        assert_has_error(&errors, "secrets.passphrase", "missing_leading_slash");
    }

    // ── Error accumulation ───────────────────────────────────────

    #[test]
    fn multiple_errors_accumulated_not_just_first() {
        let mut toml = valid_toml();
        toml = replace(&toml, "name = \"BOLT-V2-001\"", "name = \"\"");
        toml = replace(
            &toml,
            "event_slug = \"btc-updown-5m\"",
            "event_slug = \"\"",
        );
        toml = replace(&toml, "funder = \"0xabc\"", "funder = \"\"");

        let errors = errors_for(&toml);
        assert!(
            errors.len() >= 3,
            "expected at least 3 errors, got {}: {errors:?}",
            errors.len()
        );
        assert_has_error(&errors, "node.name", "empty");
        assert_has_error(&errors, "polymarket.event_slug", "empty");
        assert_has_error(&errors, "polymarket.funder", "empty");
    }

    // ── Code review fixes ──────────────────────────────────────

    #[test]
    fn event_slug_with_embedded_whitespace_rejected() {
        let toml = replace(
            &valid_toml(),
            "event_slug = \"btc-updown-5m\"",
            "event_slug = \"btc updown 5m\"",
        );
        let errors = errors_for(&toml);
        assert_has_error(&errors, "polymarket.event_slug", "contains_whitespace");
    }

    #[test]
    fn order_qty_infinity_rejected() {
        let toml = replace(&valid_toml(), "order_qty = \"5\"", "order_qty = \"inf\"");
        let errors = errors_for(&toml);
        assert_has_error(&errors, "strategy.order_qty", "not_positive_number");
    }

    #[test]
    fn empty_ssm_path_reports_empty_not_missing_slash() {
        let toml = replace(&valid_toml(), "pk = \"/bolt/poly/pk\"", "pk = \"\"");
        let errors = errors_for(&toml);
        assert_has_error(&errors, "secrets.pk", "empty");
    }

    // ── Tracked template golden path ─────────────────────────────

    #[test]
    fn tracked_template_passes_validation() {
        let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("config/live.local.example.toml");
        let contents = std::fs::read_to_string(&source).expect("tracked template should exist");
        let config: LiveLocalConfig =
            toml::from_str(&contents).expect("tracked template should parse");
        let errors = validate_live_local(&config);
        assert_no_errors(&errors);
    }

    // ══ Runtime Config validation ════════════════════════════════

    fn valid_runtime_toml() -> &'static str {
        r#"
[node]
name = "BOLT-V2-001"
trader_id = "BOLT-001"
environment = "Live"
load_state = false
save_state = false
timeout_connection_secs = 60
timeout_reconciliation_secs = 60
timeout_portfolio_secs = 10
timeout_disconnection_secs = 10
delay_post_stop_secs = 5
delay_shutdown_secs = 5

[logging]
stdout_level = "Info"
file_level = "Debug"

[[data_clients]]
name = "POLYMARKET"
type = "polymarket"
[data_clients.config]
event_slugs = ["btc-updown-5m"]

[[exec_clients]]
name = "POLYMARKET"
type = "polymarket"
[exec_clients.config]
account_id = "POLYMARKET-001"
signature_type = 2
funder = "0xabc"
[exec_clients.secrets]
region = "eu-west-1"
pk = "/bolt/poly/pk"
api_key = "/bolt/poly/key"
api_secret = "/bolt/poly/secret"
passphrase = "/bolt/poly/passphrase"

[[strategies]]
type = "exec_tester"
[strategies.config]
strategy_id = "EXEC_TESTER-001"
instrument_id = "0xabc-12345.POLYMARKET"
client_id = "POLYMARKET"
order_qty = "5"
"#
    }

    fn runtime_errors_for(toml_str: &str) -> Vec<ValidationError> {
        let config: Config =
            toml::from_str(toml_str).expect("runtime test config should parse");
        let mut errors = validate_runtime(&config);
        errors.sort();
        errors
    }

    #[test]
    fn valid_runtime_config_passes() {
        let errors = runtime_errors_for(valid_runtime_toml());
        assert_no_errors(&errors);
    }

    #[test]
    fn duplicate_data_client_names_rejected() {
        let toml = format!(
            "{}\n{}",
            valid_runtime_toml(),
            r#"
[[data_clients]]
name = "POLYMARKET"
type = "polymarket"
[data_clients.config]
event_slugs = ["other-slug"]
"#
        );
        let errors = runtime_errors_for(&toml);
        assert_has_error(&errors, "data_clients", "duplicate_name");
    }

    #[test]
    fn duplicate_exec_client_names_rejected() {
        let toml = format!(
            "{}\n{}",
            valid_runtime_toml(),
            r#"
[[exec_clients]]
name = "POLYMARKET"
type = "polymarket"
[exec_clients.config]
account_id = "POLYMARKET-002"
signature_type = 2
funder = "0xdef"
[exec_clients.secrets]
region = "eu-west-1"
pk = "/bolt/poly/pk2"
api_key = "/bolt/poly/key2"
api_secret = "/bolt/poly/secret2"
passphrase = "/bolt/poly/passphrase2"
"#
        );
        let errors = runtime_errors_for(&toml);
        assert_has_error(&errors, "exec_clients", "duplicate_name");
    }

    #[test]
    fn strategy_referencing_nonexistent_client_rejected() {
        let toml = valid_runtime_toml().replace(
            "client_id = \"POLYMARKET\"",
            "client_id = \"NONEXISTENT\"",
        );
        let errors = runtime_errors_for(&toml);
        assert_has_error(&errors, "strategies", "unknown_client_id");
    }

    #[test]
    fn strategy_missing_client_id_rejected() {
        let toml = valid_runtime_toml().replace("client_id = \"POLYMARKET\"\n", "");
        let errors = runtime_errors_for(&toml);
        assert_has_error(&errors, "strategies", "missing_client_id");
    }

    #[test]
    fn strategy_referencing_existing_client_accepted() {
        let errors = runtime_errors_for(valid_runtime_toml());
        assert!(
            !errors.iter().any(|e| e.code == "unknown_client_id"),
            "valid client_id should not produce errors"
        );
    }
}
