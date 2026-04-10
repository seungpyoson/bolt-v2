use super::*;
use std::io::Write;
use tempfile::NamedTempFile;

// ════════════════════════════════════════════════════════════════
// Test infrastructure
// ════════════════════════════════════════════════════════════════

/// Minimal valid config -- all fields satisfy validation rules.
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
    let count = base.matches(old).count();
    assert_eq!(
        count, 1,
        "test helper: '{old}' must appear exactly once in base config (found {count})"
    );
    base.replacen(old, new, 1)
}

fn parse(toml: &str) -> LiveLocalConfig {
    toml::from_str(toml).expect("test config should parse")
}

fn parse_error_for(toml: &str) -> String {
    toml::from_str::<LiveLocalConfig>(toml)
        .expect_err("test config should fail to parse")
        .to_string()
}

fn errors_for(toml: &str) -> Vec<ValidationError> {
    let config = parse(toml);
    validate_live_local(&config)
}

fn assert_has_error(errors: &[ValidationError], field: &str, code: &str) {
    assert!(
        errors.iter().any(|e| e.field == field && e.code == code),
        "expected error field={field} code={code}, got: {errors:?}"
    );
}

fn assert_error_message_contains(
    errors: &[ValidationError],
    field: &str,
    code: &str,
    needle: &str,
) {
    let error = errors
        .iter()
        .find(|e| e.field == field && e.code == code)
        .unwrap_or_else(|| panic!("expected error field={field} code={code}, got: {errors:?}"));
    assert!(
        error.message.contains(needle),
        "expected error message to contain {needle:?}, got: {:?}",
        error.message
    );
}

fn assert_error_message_not_contains(
    errors: &[ValidationError],
    field: &str,
    code: &str,
    needle: &str,
) {
    let error = errors
        .iter()
        .find(|e| e.field == field && e.code == code)
        .unwrap_or_else(|| panic!("expected error field={field} code={code}, got: {errors:?}"));
    assert!(
        !error.message.contains(needle),
        "expected error message not to contain {needle:?}, got: {:?}",
        error.message
    );
}

fn assert_no_errors(errors: &[ValidationError]) {
    assert!(errors.is_empty(), "expected no errors, got: {errors:?}");
}

// ════════════════════════════════════════════════════════════════
// Golden path
// ════════════════════════════════════════════════════════════════

#[test]
fn valid_config_passes_all_validation() {
    let errors = errors_for(&valid_toml());
    assert_no_errors(&errors);
}

#[test]
fn tracked_template_passes_validation() {
    let source =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("config/live.local.example.toml");
    let contents = std::fs::read_to_string(&source).expect("tracked template should exist");
    let config: LiveLocalConfig = toml::from_str(&contents).expect("tracked template should parse");
    let errors = validate_live_local(&config);
    assert_no_errors(&errors);
}

#[test]
fn unknown_field_in_live_local_rejected() {
    let toml = replace(
        &valid_toml(),
        "order_qty = \"5\"",
        "order_qty = \"5\"\noder_qty = \"10\"",
    );
    let error = parse_error_for(&toml);
    assert!(
        error.contains("unknown field `oder_qty`"),
        "expected serde unknown-field error, got: {error}"
    );
}

#[test]
fn missing_secrets_section_produces_validator_error() {
    let toml = valid_toml().replace(
        "\n[secrets]\npk = \"/bolt/poly/pk\"\napi_key = \"/bolt/poly/key\"\napi_secret = \"/bolt/poly/secret\"\npassphrase = \"/bolt/poly/passphrase\"\n",
        "\n",
    );
    let errors = errors_for(&toml);
    assert_has_error(&errors, "secrets.region", "empty");
    assert_has_error(&errors, "secrets.pk", "empty");
    assert_has_error(&errors, "secrets.api_key", "empty");
    assert_has_error(&errors, "secrets.api_secret", "empty");
    assert_has_error(&errors, "secrets.passphrase", "empty");
}

// ════════════════════════════════════════════════════════════════
// NT ASCII contract (check_nt_ascii)
// ════════════════════════════════════════════════════════════════

#[test]
fn empty_node_name_rejected() {
    let toml = replace(&valid_toml(), "name = \"BOLT-V2-001\"", "name = \"\"");
    let errors = errors_for(&toml);
    assert_has_error(&errors, "node.name", "empty");
}

#[test]
fn whitespace_only_node_name_rejected() {
    let toml = replace(&valid_toml(), "name = \"BOLT-V2-001\"", "name = \"   \"");
    let errors = errors_for(&toml);
    assert_has_error(&errors, "node.name", "whitespace_only");
}

#[test]
fn node_name_unicode_accepted() {
    let toml = replace(
        &valid_toml(),
        "name = \"BOLT-V2-001\"",
        "name = \"BOLT-V2-\u{00e9}\"",
    );
    let errors = errors_for(&toml);
    assert_no_errors(&errors);
}

// ════════════════════════════════════════════════════════════════
// NT NAME-TAG contract (check_nt_name_tag)
// ════════════════════════════════════════════════════════════════

#[test]
fn trader_id_without_hyphen_rejected() {
    let toml = replace(
        &valid_toml(),
        "trader_id = \"BOLT-001\"",
        "trader_id = \"BOLT001\"",
    );
    let errors = errors_for(&toml);
    assert_has_error(&errors, "node.trader_id", "missing_hyphen");
    assert_error_message_contains(
        &errors,
        "node.trader_id",
        "missing_hyphen",
        "must contain a hyphen separating name and tag, got \"BOLT001\"",
    );
    assert_error_message_contains(
        &errors,
        "node.trader_id",
        "missing_hyphen",
        "(example: \"NAME-TAG\")",
    );
    assert_error_message_not_contains(
        &errors,
        "node.trader_id",
        "missing_hyphen",
        "node.trader_id must",
    );
}

#[test]
fn trader_id_with_empty_name_part_rejected() {
    let toml = replace(
        &valid_toml(),
        "trader_id = \"BOLT-001\"",
        "trader_id = \"-001\"",
    );
    let errors = errors_for(&toml);
    assert_has_error(&errors, "node.trader_id", "empty_name_part");
}

#[test]
fn trader_id_with_empty_tag_part_rejected() {
    let toml = replace(
        &valid_toml(),
        "trader_id = \"BOLT-001\"",
        "trader_id = \"BOLT-\"",
    );
    let errors = errors_for(&toml);
    assert_has_error(&errors, "node.trader_id", "empty_tag_part");
}

#[test]
fn trader_id_trailing_hyphen_rejected_via_rsplit() {
    // "BOLT-V2-" → rsplit_once('-') → ("BOLT-V2", "") → empty tag
    // split_once('-') would give ("BOLT", "V2-") which wrongly passes.
    let toml = replace(
        &valid_toml(),
        "trader_id = \"BOLT-001\"",
        "trader_id = \"BOLT-V2-\"",
    );
    let errors = errors_for(&toml);
    assert_has_error(&errors, "node.trader_id", "empty_tag_part");
}

#[test]
fn trader_id_multi_hyphen_accepted() {
    // "BOLT-V2-001" is valid: rsplit_once('-') → ("BOLT-V2", "001")
    let toml = replace(
        &valid_toml(),
        "trader_id = \"BOLT-001\"",
        "trader_id = \"BOLT-V2-001\"",
    );
    let errors = errors_for(&toml);
    assert_no_errors(&errors);
}

#[test]
fn account_id_without_hyphen_rejected() {
    let toml = replace(
        &valid_toml(),
        "account_id = \"POLYMARKET-001\"",
        "account_id = \"POLYMARKET001\"",
    );
    let errors = errors_for(&toml);
    assert_has_error(&errors, "polymarket.account_id", "missing_hyphen");
    assert_error_message_contains(
        &errors,
        "polymarket.account_id",
        "missing_hyphen",
        "(example: \"ISSUER-ACCOUNT\")",
    );
}

#[test]
fn strategy_id_without_hyphen_rejected() {
    let toml = replace(
        &valid_toml(),
        "strategy_id = \"EXEC_TESTER-001\"",
        "strategy_id = \"EXECTESTER001\"",
    );
    let errors = errors_for(&toml);
    assert_has_error(&errors, "strategy.strategy_id", "missing_hyphen");
}

#[test]
fn strategy_id_external_accepted() {
    let toml = replace(
        &valid_toml(),
        "strategy_id = \"EXEC_TESTER-001\"",
        "strategy_id = \"EXTERNAL\"",
    );
    let errors = errors_for(&toml);
    assert!(
        !errors.iter().any(|e| e.field == "strategy.strategy_id"),
        "EXTERNAL should not produce strategy_id errors, got: {errors:?}"
    );
}

// ════════════════════════════════════════════════════════════════
// Instrument ID
// ════════════════════════════════════════════════════════════════

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
    assert_has_error(&errors, "polymarket.instrument_id", "missing_venue_suffix");
    assert_error_message_contains(
        &errors,
        "polymarket.instrument_id",
        "missing_venue_suffix",
        "(example: \"0xabc-12345.POLYMARKET\")",
    );
}

#[test]
fn instrument_id_bare_suffix_rejected() {
    let toml = replace(
        &valid_toml(),
        "instrument_id = \"0xabc-12345.POLYMARKET\"",
        "instrument_id = \".POLYMARKET\"",
    );
    let errors = errors_for(&toml);
    assert_has_error(&errors, "polymarket.instrument_id", "empty_symbol");
}

#[test]
fn instrument_id_whitespace_symbol_rejected() {
    let toml = replace(
        &valid_toml(),
        "instrument_id = \"0xabc-12345.POLYMARKET\"",
        "instrument_id = \"   .POLYMARKET\"",
    );
    let errors = errors_for(&toml);
    assert_has_error(
        &errors,
        "polymarket.instrument_id",
        "whitespace_only_symbol",
    );
}

// ════════════════════════════════════════════════════════════════
// Quantity (check_positive_qty)
// ════════════════════════════════════════════════════════════════

#[test]
fn order_qty_non_numeric_rejected() {
    let toml = replace(&valid_toml(), "order_qty = \"5\"", "order_qty = \"abc\"");
    let errors = errors_for(&toml);
    assert_has_error(&errors, "strategy.order_qty", "not_parseable");
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
    assert_has_error(&errors, "strategy.order_qty", "not_parseable");
}

#[test]
fn order_qty_infinity_rejected() {
    let toml = replace(&valid_toml(), "order_qty = \"5\"", "order_qty = \"inf\"");
    let errors = errors_for(&toml);
    assert_has_error(&errors, "strategy.order_qty", "not_parseable");
}

#[test]
fn order_qty_with_underscores_accepted() {
    let toml = replace(&valid_toml(), "order_qty = \"5\"", "order_qty = \"1_000\"");
    let errors = errors_for(&toml);
    assert_no_errors(&errors);
}

#[test]
fn order_qty_scientific_notation_accepted() {
    let toml = replace(&valid_toml(), "order_qty = \"5\"", "order_qty = \"1e3\"");
    let errors = errors_for(&toml);
    assert_no_errors(&errors);
}

#[test]
fn order_qty_high_precision_accepted() {
    let toml = replace(
        &valid_toml(),
        "order_qty = \"5\"",
        "order_qty = \"0.0000000001\"",
    );
    let errors = errors_for(&toml);
    assert_no_errors(&errors);
}

#[test]
fn order_qty_scientific_negative_exponent_accepted() {
    let toml = replace(&valid_toml(), "order_qty = \"5\"", "order_qty = \"1e-10\"");
    let errors = errors_for(&toml);
    assert_no_errors(&errors);
}

#[test]
fn order_qty_precision_error_includes_nt_diagnostic() {
    let toml = replace(
        &valid_toml(),
        "order_qty = \"5\"",
        "order_qty = \"0.12345678901234567\"",
    );
    let errors = errors_for(&toml);
    assert_has_error(&errors, "strategy.order_qty", "not_parseable");

    let error = errors
        .iter()
        .find(|e| e.field == "strategy.order_qty" && e.code == "not_parseable")
        .expect("expected quantity parse error");
    assert!(
        error.message.contains("precision") || error.message.contains("FIXED_PRECISION"),
        "expected NT diagnostic in parse error, got: {:?}",
        error.message
    );
}

// ════════════════════════════════════════════════════════════════
// New fields: client_name, environment, log_level
// ════════════════════════════════════════════════════════════════

#[test]
fn empty_client_name_rejected() {
    let toml = replace(
        &valid_toml(),
        "[polymarket]",
        "[polymarket]\nclient_name = \"\"",
    );
    let errors = errors_for(&toml);
    assert_has_error(&errors, "polymarket.client_name", "empty");
}

#[test]
fn invalid_environment_rejected() {
    let toml = replace(&valid_toml(), "[node]", "[node]\nenvironment = \"live\"");
    let errors = errors_for(&toml);
    assert_has_error(&errors, "node.environment", "invalid_environment");
}

#[test]
fn invalid_log_level_rejected() {
    let toml = replace(
        &valid_toml(),
        "[strategy]",
        "[logging]\nstdout_level = \"info\"\nfile_level = \"debug\"\n\n[strategy]",
    );
    let errors = errors_for(&toml);
    assert_has_error(&errors, "logging.stdout_level", "invalid_log_level");
    assert_has_error(&errors, "logging.file_level", "invalid_log_level");
}

#[test]
fn signature_type_out_of_range_rejected() {
    let toml = replace(
        &valid_toml(),
        "funder = \"0xabc\"",
        "funder = \"0xabc\"\nsignature_type = 3",
    );
    let errors = errors_for(&toml);
    assert_has_error(
        &errors,
        "polymarket.signature_type",
        "invalid_signature_type",
    );
    assert_error_message_contains(
        &errors,
        "polymarket.signature_type",
        "invalid_signature_type",
        "(valid values: 0, 1, 2)",
    );
}

#[test]
fn timeouts_zero_rejected() {
    let toml = replace(
        &valid_toml(),
        "[polymarket]",
        r#"[timeouts]
connection_secs = 0
reconciliation_secs = 0
portfolio_secs = 0
disconnection_secs = 0
post_stop_delay_secs = 0
shutdown_delay_secs = 0

[polymarket]"#,
    );
    let errors = errors_for(&toml);
    for field in [
        "timeouts.connection_secs",
        "timeouts.reconciliation_secs",
        "timeouts.portfolio_secs",
        "timeouts.disconnection_secs",
        "timeouts.post_stop_delay_secs",
        "timeouts.shutdown_delay_secs",
    ] {
        assert_has_error(&errors, field, "not_positive");
    }
}

#[test]
fn flush_interval_zero_with_catalog_path_rejected() {
    let toml = replace(
        &valid_toml(),
        "[strategy]",
        "[streaming]\ncatalog_path = \"/data\"\nflush_interval_ms = 0\n\n[strategy]",
    );
    let errors = errors_for(&toml);
    assert_has_error(&errors, "streaming.flush_interval_ms", "not_positive");
}

#[test]
fn contract_path_requires_streaming_catalog_path() {
    let toml = replace(
        &valid_toml(),
        "[strategy]",
        "[streaming]\ncatalog_path = \"\"\ncontract_path = \"contracts/polymarket.toml\"\n\n[strategy]",
    );
    let errors = errors_for(&toml);
    assert_has_error(&errors, "streaming.contract_path", "requires_catalog_path");
}

#[test]
fn empty_contract_path_rejected() {
    let toml = replace(
        &valid_toml(),
        "[strategy]",
        "[streaming]\ncatalog_path = \"var/catalog\"\ncontract_path = \"\"\n\n[strategy]",
    );
    let errors = errors_for(&toml);
    assert_has_error(&errors, "streaming.contract_path", "empty");
}

#[test]
fn live_local_non_local_contract_path_rejected_before_render() {
    let toml = replace(
        &valid_toml(),
        "[strategy]",
        "[streaming]\ncatalog_path = \"var/catalog\"\ncontract_path = \"s3://bucket/contracts/polymarket.toml\"\n\n[strategy]",
    );
    let errors = errors_for(&toml);
    assert_has_error(&errors, "streaming.contract_path", "non_local");
    assert_error_message_contains(
        &errors,
        "streaming.contract_path",
        "non_local",
        "local path",
    );
    assert_error_message_not_contains(&errors, "streaming.contract_path", "non_local", "absolute");
}

#[test]
fn live_local_relative_contract_path_remains_valid() {
    let toml = replace(
        &valid_toml(),
        "[strategy]",
        "[streaming]\ncatalog_path = \"var/catalog\"\ncontract_path = \"contracts/polymarket.toml\"\n\n[strategy]",
    );
    let errors = errors_for(&toml);
    assert_no_errors(&errors);
}

// ════════════════════════════════════════════════════════════════
// Domain-specific: event_slug, funder, SSM paths
// ════════════════════════════════════════════════════════════════

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
    assert_error_message_contains(
        &errors,
        "polymarket.funder",
        "missing_hex_prefix",
        "(example: \"0xabc...\")",
    );
}

#[test]
fn ssm_path_missing_leading_slash_rejected() {
    let toml = replace(
        &valid_toml(),
        "pk = \"/bolt/poly/pk\"",
        "pk = \"bolt/poly/pk\"",
    );
    let errors = errors_for(&toml);
    assert_has_error(&errors, "secrets.pk", "missing_leading_slash");
    assert_error_message_contains(
        &errors,
        "secrets.pk",
        "missing_leading_slash",
        "(example: \"/bolt/poly/pk\")",
    );
}

#[test]
fn empty_ssm_path_reports_empty_not_missing_slash() {
    let toml = replace(&valid_toml(), "pk = \"/bolt/poly/pk\"", "pk = \"\"");
    let errors = errors_for(&toml);
    assert_has_error(&errors, "secrets.pk", "empty");
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

#[test]
fn secrets_region_empty_rejected() {
    let toml = replace(&valid_toml(), "[secrets]", "[secrets]\nregion = \"\"");
    let errors = errors_for(&toml);
    assert_has_error(&errors, "secrets.region", "empty");
}

// ════════════════════════════════════════════════════════════════
// Error accumulation
// ════════════════════════════════════════════════════════════════

#[test]
fn multiple_errors_accumulated_not_just_first() {
    let mut toml = valid_toml();
    toml = replace(&toml, "name = \"BOLT-V2-001\"", "name = \"\"");
    toml = replace(&toml, "event_slug = \"btc-updown-5m\"", "event_slug = \"\"");
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

// ════════════════════════════════════════════════════════════════
// Phase 1 render-time validation
// ════════════════════════════════════════════════════════════════

// ════════════════════════════════════════════════════════════════
// Runtime config validation
// ════════════════════════════════════════════════════════════════

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
    let config: Config = toml::from_str(toml_str).expect("runtime test config should parse");
    validate_runtime(&config)
}

fn runtime_load_error_for(toml_str: &str) -> String {
    let mut file = NamedTempFile::new().expect("runtime temp file should be created");
    file.write_all(toml_str.as_bytes())
        .expect("runtime temp file should be written");
    Config::load(file.path())
        .expect_err("runtime config should fail validation")
        .to_string()
}

#[test]
fn valid_runtime_config_passes() {
    let errors = runtime_errors_for(valid_runtime_toml());
    assert_no_errors(&errors);
}

#[test]
fn runtime_event_slugs_wrong_type_rejected() {
    let toml = valid_runtime_toml().replace(
        "event_slugs = [\"btc-updown-5m\"]",
        "event_slugs = \"not-an-array\"",
    );
    let errors = runtime_errors_for(&toml);
    assert_has_error(&errors, "data_clients[0].config.event_slugs", "wrong_type");
    assert_error_message_contains(
        &errors,
        "data_clients[0].config.event_slugs",
        "wrong_type",
        "must be an array, got string value",
    );
}

#[test]
fn runtime_empty_event_slugs_rejected() {
    let toml =
        valid_runtime_toml().replace("event_slugs = [\"btc-updown-5m\"]", "event_slugs = []");
    let errors = runtime_errors_for(&toml);
    assert_has_error(&errors, "data_clients[0].config.event_slugs", "empty");
}

#[test]
fn runtime_invalid_trader_id_rejected() {
    let toml = valid_runtime_toml().replace("trader_id = \"BOLT-001\"", "trader_id = \"BOLT001\"");
    let errors = runtime_errors_for(&toml);
    assert_has_error(&errors, "node.trader_id", "missing_hyphen");
}

#[test]
fn runtime_signature_type_wrong_type_rejected() {
    let toml = valid_runtime_toml().replace("signature_type = 2", "signature_type = \"2\"");
    let errors = runtime_errors_for(&toml);
    assert_has_error(
        &errors,
        "exec_clients[0].config.signature_type",
        "wrong_type",
    );
    assert_error_message_contains(
        &errors,
        "exec_clients[0].config.signature_type",
        "wrong_type",
        "must be an integer, got string value",
    );
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
    assert_error_message_contains(
        &errors,
        "data_clients",
        "duplicate_name",
        "first defined at data_clients[0]",
    );
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
    assert_error_message_contains(
        &errors,
        "exec_clients",
        "duplicate_name",
        "first defined at exec_clients[0]",
    );
}

#[test]
fn runtime_empty_client_name_rejected() {
    let toml = valid_runtime_toml()
        .replacen("name = \"POLYMARKET\"", "name = \"\"", 1)
        .replacen("name = \"POLYMARKET\"", "name = \"\"", 1);
    let errors = runtime_errors_for(&toml);
    assert_has_error(&errors, "data_clients[0].name", "empty");
    assert_has_error(&errors, "exec_clients[0].name", "empty");
}

#[test]
fn duplicate_strategy_id_names_first_occurrence() {
    let toml = format!(
        "{}\n{}\n{}",
        valid_runtime_toml(),
        r#"
[[strategies]]
type = "exec_tester"
[strategies.config]
strategy_id = "EXEC_TESTER-001"
instrument_id = "0xdef-67890.POLYMARKET"
client_id = "POLYMARKET"
order_qty = "10"
"#,
        r#"
[[strategies]]
type = "exec_tester"
[strategies.config]
strategy_id = "EXEC_TESTER-001"
instrument_id = "0xghi-13579.POLYMARKET"
client_id = "POLYMARKET"
order_qty = "15"
"#
    );
    let errors = runtime_errors_for(&toml);
    assert_has_error(&errors, "strategies", "duplicate_strategy_id");
    assert_error_message_contains(
        &errors,
        "strategies",
        "duplicate_strategy_id",
        "strategies[1] has duplicate strategy_id \"EXEC_TESTER-001\"",
    );
    assert!(
        errors.iter().any(|e| {
            e.field == "strategies"
                && e.code == "duplicate_strategy_id"
                && e.message
                    .contains("strategies[2] has duplicate strategy_id \"EXEC_TESTER-001\" (first defined at strategies[0])")
        }),
        "expected third duplicate to reference the original first occurrence, got: {errors:?}"
    );
    assert_error_message_contains(
        &errors,
        "strategies",
        "duplicate_strategy_id",
        "first defined at strategies[0]",
    );
}

#[test]
fn strategy_referencing_nonexistent_client_rejected() {
    let toml =
        valid_runtime_toml().replace("client_id = \"POLYMARKET\"", "client_id = \"NONEXISTENT\"");
    let errors = runtime_errors_for(&toml);
    assert_has_error(&errors, "strategies", "unknown_client_id");
}

#[test]
fn strategy_missing_client_id_rejected() {
    let toml = valid_runtime_toml().replace("client_id = \"POLYMARKET\"\n", "");
    let errors = runtime_errors_for(&toml);
    assert_has_error(
        &errors,
        "strategies[0].config.client_id",
        "missing_client_id",
    );
}

#[test]
fn strategy_referencing_existing_client_accepted() {
    let errors = runtime_errors_for(valid_runtime_toml());
    assert!(
        !errors.iter().any(|e| e.code == "unknown_client_id"),
        "valid client_id should not produce errors"
    );
}

#[test]
fn runtime_missing_strategy_id_rejected() {
    let toml = valid_runtime_toml().replace("strategy_id = \"EXEC_TESTER-001\"\n", "");
    let errors = runtime_errors_for(&toml);
    assert_has_error(
        &errors,
        "strategies[0].config.strategy_id",
        "missing_strategy_id",
    );
}

#[test]
fn runtime_strategy_id_wrong_type_rejected() {
    let toml =
        valid_runtime_toml().replace("strategy_id = \"EXEC_TESTER-001\"", "strategy_id = 42");
    let errors = runtime_errors_for(&toml);
    assert_has_error(&errors, "strategies[0].config.strategy_id", "wrong_type");
    assert_error_message_contains(
        &errors,
        "strategies[0].config.strategy_id",
        "wrong_type",
        "must be a string, got integer value",
    );
}

#[test]
fn runtime_missing_instrument_id_rejected() {
    let toml = valid_runtime_toml().replace("instrument_id = \"0xabc-12345.POLYMARKET\"\n", "");
    let errors = runtime_errors_for(&toml);
    assert_has_error(
        &errors,
        "strategies[0].config.instrument_id",
        "missing_instrument_id",
    );
}

#[test]
fn runtime_missing_order_qty_rejected() {
    let toml = valid_runtime_toml().replace("order_qty = \"5\"\n", "");
    let errors = runtime_errors_for(&toml);
    assert_has_error(
        &errors,
        "strategies[0].config.order_qty",
        "missing_order_qty",
    );
}

#[test]
fn runtime_missing_strategy_field_uses_indexed_path() {
    let toml = valid_runtime_toml().replace("client_id = \"POLYMARKET\"\n", "");
    let errors = runtime_errors_for(&toml);
    assert_has_error(
        &errors,
        "strategies[0].config.client_id",
        "missing_client_id",
    );
    assert!(
        !errors
            .iter()
            .any(|e| e.field == "strategies" && e.code == "missing_client_id"),
        "missing client_id should use indexed field path, got: {errors:?}"
    );
}

#[test]
fn runtime_invalid_instrument_id_rejected() {
    let toml = valid_runtime_toml().replace(
        "instrument_id = \"0xabc-12345.POLYMARKET\"",
        "instrument_id = \"TOKEN.TEST\"",
    );
    let errors = runtime_errors_for(&toml);
    assert_has_error(
        &errors,
        "strategies[0].config.instrument_id",
        "missing_venue_suffix",
    );
}

#[test]
fn runtime_invalid_order_qty_rejected() {
    let toml = valid_runtime_toml().replace("order_qty = \"5\"", "order_qty = \"0\"");
    let errors = runtime_errors_for(&toml);
    assert_has_error(
        &errors,
        "strategies[0].config.order_qty",
        "not_positive_number",
    );
}

#[test]
fn runtime_multiple_errors_accumulated() {
    let mut toml = valid_runtime_toml().to_string();
    toml = toml.replace("trader_id = \"BOLT-001\"", "trader_id = \"BOLT001\"");
    toml = toml.replace("region = \"eu-west-1\"", "region = \"\"");
    toml = toml.replace("signature_type = 2", "signature_type = 9");

    let errors = runtime_errors_for(&toml);
    assert!(
        errors.len() >= 3,
        "expected at least 3 runtime errors, got {}: {errors:?}",
        errors.len()
    );
    assert_has_error(&errors, "node.trader_id", "missing_hyphen");
    assert_has_error(&errors, "exec_clients[0].secrets.region", "empty");
    assert_has_error(
        &errors,
        "exec_clients[0].config.signature_type",
        "invalid_signature_type",
    );
}

#[test]
fn runtime_flush_interval_zero_with_catalog_path_rejected() {
    let toml = format!(
        "{}\n{}",
        valid_runtime_toml(),
        "[streaming]\ncatalog_path = \"var/catalog\"\nflush_interval_ms = 0\n"
    );
    let errors = runtime_errors_for(&toml);
    assert_has_error(&errors, "streaming.flush_interval_ms", "not_positive");
}

#[test]
fn runtime_contract_path_requires_streaming_catalog_path() {
    let toml = format!(
        "{}\n{}",
        valid_runtime_toml(),
        "[streaming]\ncatalog_path = \"\"\nflush_interval_ms = 1000\ncontract_path = \"/opt/bolt-v2/contracts/polymarket.toml\"\n"
    );
    let errors = runtime_errors_for(&toml);
    assert_has_error(&errors, "streaming.contract_path", "requires_catalog_path");
}

#[test]
fn runtime_relative_contract_path_rejected() {
    let toml = format!(
        "{}\n{}",
        valid_runtime_toml(),
        "[streaming]\ncatalog_path = \"var/catalog\"\nflush_interval_ms = 1000\ncontract_path = \"contracts/polymarket.toml\"\n"
    );
    let errors = runtime_errors_for(&toml);
    assert_has_error(&errors, "streaming.contract_path", "not_absolute");
}

#[test]
fn runtime_non_local_contract_path_rejected() {
    let toml = format!(
        "{}\n{}",
        valid_runtime_toml(),
        "[streaming]\ncatalog_path = \"var/catalog\"\nflush_interval_ms = 1000\ncontract_path = \"s3://bucket/contracts/polymarket.toml\"\n"
    );
    let errors = runtime_errors_for(&toml);
    assert_has_error(&errors, "streaming.contract_path", "non_local");
    assert_error_message_contains(
        &errors,
        "streaming.contract_path",
        "non_local",
        "local path",
    );
    assert_error_message_not_contains(&errors, "streaming.contract_path", "non_local", "absolute");
}

#[test]
fn runtime_empty_contract_path_rejected() {
    let toml = format!(
        "{}\n{}",
        valid_runtime_toml(),
        "[streaming]\ncatalog_path = \"var/catalog\"\nflush_interval_ms = 1000\ncontract_path = \"\"\n"
    );
    let errors = runtime_errors_for(&toml);
    assert_has_error(&errors, "streaming.contract_path", "empty");
}

#[test]
fn runtime_load_rejects_relative_contract_path() {
    let toml = format!(
        "{}\n{}",
        valid_runtime_toml(),
        "[streaming]\ncatalog_path = \"var/catalog\"\nflush_interval_ms = 1000\ncontract_path = \"contracts/polymarket.toml\"\n"
    );
    let error = runtime_load_error_for(&toml);
    assert!(
        error.contains("streaming.contract_path"),
        "runtime load error should mention contract_path: {error}"
    );
    assert!(
        error.contains("local absolute path"),
        "runtime load error should mention local absolute path: {error}"
    );
}

#[test]
fn runtime_unsupported_client_type_rejected() {
    let toml = valid_runtime_toml()
        .replacen("type = \"polymarket\"", "type = \"bogus\"", 1)
        .replacen("type = \"polymarket\"", "type = \"bogus\"", 1)
        .replacen("type = \"exec_tester\"", "type = \"bogus\"", 1);
    let errors = runtime_errors_for(&toml);
    assert_has_error(&errors, "data_clients[0].type", "unsupported_type");
    assert_has_error(&errors, "exec_clients[0].type", "unsupported_type");
    assert_has_error(&errors, "strategies[0].type", "unsupported_type");
}

#[test]
fn runtime_validation_via_config_load() {
    let toml = valid_runtime_toml().replace("trader_id = \"BOLT-001\"", "trader_id = \"BOLT001\"");
    let error = runtime_load_error_for(&toml);
    assert!(
        error.contains("Runtime config validation failed"),
        "unexpected load error: {error}"
    );
    assert!(
        error.contains("node.trader_id"),
        "runtime load error should mention trader_id: {error}"
    );
}
