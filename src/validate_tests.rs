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
#[test]
fn phase1_ruleset_venue_unknown_value_rejected_during_parse() {
    let toml = replace(
        &valid_phase1_toml(),
        "venue = \"polymarket\"",
        "venue = \"kalshi\"",
    );
    let error = parse_error_for(&toml);
    assert!(
        error.contains("unknown variant"),
        "unexpected parse error: {error}"
    );
    assert!(
        error.contains("kalshi"),
        "parse error should mention invalid venue: {error}"
    );
}

#[test]
fn phase1_audit_required_when_rulesets_are_configured() {
    let toml = valid_phase1_toml()
        .replace("[audit]\n", "")
        .replace("local_dir = \"var/audit\"\n", "")
        .replace("s3_uri = \"s3://bolt-runtime-history/phase1\"\n", "")
        .replace("ship_interval_secs = 30\n", "")
        .replace("roll_max_bytes = 1048576\n", "")
        .replace("roll_max_secs = 300\n", "")
        .replace("max_local_backlog_bytes = 10485760\n", "");
    let errors = errors_for(&toml);
    assert_has_error(&errors, "audit", "missing_audit");
}

#[test]
fn phase1_duplicate_ruleset_ids_rejected() {
    let toml = format!(
        "{}\n{}",
        valid_phase1_toml(),
        r#"
[[rulesets]]
id = "PRIMARY"
venue = "polymarket"
tag_slug = "bitcoin-2"
resolution_basis = "binance_btcusdt_5m"
min_time_to_expiry_secs = 60
max_time_to_expiry_secs = 900
min_liquidity_num = 1000
require_accepting_orders = true
freeze_before_end_secs = 30
"#
    );
    let errors = errors_for(&toml);
    assert_has_error(&errors, "rulesets", "duplicate_ruleset_id");
}

#[test]
fn phase1_reference_venue_name_must_be_non_empty() {
    let toml = replace(
        &valid_phase1_toml(),
        "name = \"BINANCE-BTC\"",
        "name = \"\"",
    );
    let errors = errors_for(&toml);
    assert_has_error(&errors, "reference.venues[0].name", "empty");
}

#[test]
fn phase1_reference_venue_weight_must_be_positive_and_finite() {
    let toml = replace(
        &valid_phase1_toml(),
        "base_weight = 0.35",
        "base_weight = 0.0",
    );
    let errors = errors_for(&toml);
    assert_has_error(
        &errors,
        "reference.venues[0].base_weight",
        "not_positive_finite",
    );
}

#[test]
fn phase1_reference_venue_stale_after_ms_must_be_positive() {
    let toml = replace(
        &valid_phase1_toml(),
        "stale_after_ms = 1500",
        "stale_after_ms = 0",
    );
    let errors = errors_for(&toml);
    assert_has_error(
        &errors,
        "reference.venues[0].stale_after_ms",
        "not_positive",
    );
}

#[test]
fn phase1_reference_venue_disable_after_ms_must_not_precede_stale_after_ms() {
    let toml = replace(
        &valid_phase1_toml(),
        "disable_after_ms = 5000",
        "disable_after_ms = 1000",
    );
    let errors = errors_for(&toml);
    assert_has_error(
        &errors,
        "reference.venues[0].disable_after_ms",
        "invalid_disable_after_ms",
    );
}
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

fn valid_phase1_toml() -> String {
    format!(
        "{}\n{}",
        valid_toml(),
        r#"
[reference]
publish_topic = "platform.reference.default"
min_publish_interval_ms = 100

[[reference.venues]]
name = "BINANCE-BTC"
type = "binance"
instrument_id = "BTCUSDT.BINANCE"
base_weight = 0.35
stale_after_ms = 1500
disable_after_ms = 5000

[[rulesets]]
id = "PRIMARY"
venue = "polymarket"
tag_slug = "bitcoin"
resolution_basis = "binance_btcusdt_1m"
min_time_to_expiry_secs = 60
max_time_to_expiry_secs = 900
min_liquidity_num = 1000
require_accepting_orders = true
freeze_before_end_secs = 30

[audit]
local_dir = "var/audit"
s3_uri = "s3://bolt-runtime-history/phase1"
ship_interval_secs = 30
roll_max_bytes = 1048576
roll_max_secs = 300
max_local_backlog_bytes = 10485760
"#
    )
}

fn valid_phase1_runtime_toml() -> String {
    format!(
        "{}\n{}",
        valid_runtime_toml(),
        r#"
[reference]
publish_topic = "platform.reference.default"
min_publish_interval_ms = 100

[[reference.venues]]
name = "BINANCE-BTC"
type = "binance"
instrument_id = "BTCUSDT.BINANCE"
base_weight = 0.35
stale_after_ms = 1500
disable_after_ms = 5000

[[rulesets]]
id = "PRIMARY"
venue = "polymarket"
tag_slug = "bitcoin"
resolution_basis = "binance_btcusdt_1m"
min_time_to_expiry_secs = 60
max_time_to_expiry_secs = 900
min_liquidity_num = 1000
require_accepting_orders = true
freeze_before_end_secs = 30

[audit]
local_dir = "var/audit"
s3_uri = "s3://bolt-runtime-history/phase1"
ship_interval_secs = 30
roll_max_bytes = 1048576
roll_max_secs = 300
max_local_backlog_bytes = 10485760
"#
    )
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

#[test]
fn runtime_missing_strategy_id_rejected() {
    let toml = valid_runtime_toml().replace("strategy_id = \"EXEC_TESTER-001\"\n", "");
    let errors = runtime_errors_for(&toml);
    assert_has_error(&errors, "strategies", "missing_strategy_id");
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
    assert_has_error(&errors, "strategies", "missing_instrument_id");
}

#[test]
fn runtime_missing_order_qty_rejected() {
    let toml = valid_runtime_toml().replace("order_qty = \"5\"\n", "");
    let errors = runtime_errors_for(&toml);
    assert_has_error(&errors, "strategies", "missing_order_qty");
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

#[test]
fn phase1_runtime_requires_exactly_one_active_ruleset() {
    let toml = format!(
        "{}\n{}",
        valid_phase1_runtime_toml(),
        r#"
[[rulesets]]
id = "SECONDARY"
venue = "polymarket"
tag_slug = "bitcoin-2"
resolution_basis = "binance_btcusdt_5m"
min_time_to_expiry_secs = 60
max_time_to_expiry_secs = 900
min_liquidity_num = 1000
require_accepting_orders = true
freeze_before_end_secs = 30
"#
    );
    let errors = runtime_errors_for(&toml);
    assert_has_error(&errors, "rulesets", "phase1_single_active_ruleset");
}

#[test]
fn phase1_runtime_requires_reference_venues_when_one_ruleset_is_active() {
    let toml = valid_phase1_runtime_toml()
        .replace("[reference]\n", "")
        .replace("publish_topic = \"platform.reference.default\"\n", "")
        .replace("min_publish_interval_ms = 100\n", "")
        .replace("[[reference.venues]]\n", "")
        .replace("name = \"BINANCE-BTC\"\n", "")
        .replace("type = \"binance\"\n", "")
        .replace("instrument_id = \"BTCUSDT.BINANCE\"\n", "")
        .replace("base_weight = 0.35\n", "")
        .replace("stale_after_ms = 1500\n", "")
        .replace("disable_after_ms = 5000\n", "");
    let errors = runtime_errors_for(&toml);
    assert_has_error(&errors, "reference.venues", "missing_reference_venues");
}

#[test]
fn phase1_runtime_rejects_duplicate_reference_venue_names() {
    let toml = format!(
        "{}\n{}",
        valid_phase1_runtime_toml(),
        r#"
[[reference.venues]]
name = "BINANCE-BTC"
type = "deribit"
instrument_id = "BTC-PERPETUAL.DERIBIT"
base_weight = 0.20
stale_after_ms = 1500
disable_after_ms = 5000
"#
    );
    let errors = runtime_errors_for(&toml);
    assert_has_error(&errors, "reference.venues", "duplicate_name");
}

#[test]
fn phase1_runtime_resolution_basis_requires_matching_reference_venue_family() {
    let toml = replace(
        &valid_phase1_runtime_toml(),
        "resolution_basis = \"binance_btcusdt_1m\"",
        "resolution_basis = \"kraken_btcusd_1m\"",
    );
    let errors = runtime_errors_for(&toml);
    assert_has_error(
        &errors,
        "rulesets[0].resolution_basis",
        "missing_reference_venue_family",
    );
}

#[test]
fn phase1_runtime_polymarket_reference_must_reuse_primary_client() {
    let toml = format!(
        "{}\n{}",
        valid_phase1_runtime_toml(),
        r#"
[[reference.venues]]
name = "POLY-REF"
type = "polymarket"
instrument_id = "0xabc-12345.POLYMARKET"
base_weight = 0.25
stale_after_ms = 1500
disable_after_ms = 5000

[[data_clients]]
name = "POLYMARKET-REF"
type = "polymarket"
[data_clients.config]
event_slugs = ["btc-updown-5m"]
"#
    );
    let errors = runtime_errors_for(&toml);
    assert_has_error(
        &errors,
        "data_clients",
        "duplicate_polymarket_client_for_reference",
    );
}
