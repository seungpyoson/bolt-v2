use std::{cell::RefCell, rc::Rc};

use super::*;
use crate::strategies::registry::{
    BoxedStrategy, StrategyBuildContext, StrategyBuilder, StrategyRegistry,
};
use anyhow::anyhow;
use nautilus_model::identifiers::StrategyId;
use nautilus_system::trader::Trader;
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

[secrets]
pk = "/bolt/poly/pk"
api_key = "/bolt/poly/key"
api_secret = "/bolt/poly/secret"
passphrase = "/bolt/poly/passphrase"

[raw_capture]
output_dir = "/srv/bolt-v2/var/raw"
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

fn strip_block(base: &str, block: &str) -> String {
    let count = base.matches(block).count();
    assert_eq!(
        count, 1,
        "test helper: block must appear exactly once in base config (found {count})"
    );
    base.replacen(block, "", 1)
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

fn assert_lacks_error(errors: &[ValidationError], field: &str, code: &str) {
    assert!(
        !errors.iter().any(|e| e.field == field && e.code == code),
        "expected no error field={field} code={code}, got: {errors:?}"
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
    let tracked = std::path::Path::new("config/live.local.example.toml");
    let source = if tracked.exists() {
        std::env::current_dir()
            .expect("current_dir should resolve for tests")
            .join(tracked)
    } else {
        let output = std::process::Command::new("git")
            .args(["rev-parse", "--show-toplevel"])
            .output()
            .expect("git should be available for repo-root lookup");
        assert!(
            output.status.success(),
            "git rev-parse --show-toplevel failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        std::path::PathBuf::from(
            String::from_utf8(output.stdout)
                .expect("git output should be utf-8")
                .trim()
                .to_string(),
        )
        .join("config/live.local.example.toml")
    };
    let contents = std::fs::read_to_string(&source).expect("tracked template should exist");
    let config: LiveLocalConfig = toml::from_str(&contents).expect("tracked template should parse");
    let errors = validate_live_local(&config);
    assert_no_errors(&errors);
}

#[test]
fn unknown_field_in_live_local_rejected() {
    let toml = replace(
        &valid_toml(),
        "pk = \"/bolt/poly/pk\"",
        "pk = \"/bolt/poly/pk\"\noder_qty = \"10\"",
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
        "[secrets]",
        "[logging]\nstdout_level = \"info\"\nfile_level = \"debug\"\n\n[secrets]",
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
        "[secrets]",
        "[streaming]\ncatalog_path = \"/data\"\nflush_interval_ms = 0\n\n[secrets]",
    );
    let errors = errors_for(&toml);
    assert_has_error(&errors, "streaming.flush_interval_ms", "not_positive");
}

#[test]
fn contract_path_requires_streaming_catalog_path() {
    let toml = replace(
        &valid_toml(),
        "[secrets]",
        "[streaming]\ncatalog_path = \"\"\ncontract_path = \"contracts/polymarket.toml\"\n\n[secrets]",
    );
    let errors = errors_for(&toml);
    assert_has_error(&errors, "streaming.contract_path", "requires_catalog_path");
}

#[test]
fn empty_contract_path_rejected() {
    let toml = replace(
        &valid_toml(),
        "[secrets]",
        "[streaming]\ncatalog_path = \"var/catalog\"\ncontract_path = \"\"\n\n[secrets]",
    );
    let errors = errors_for(&toml);
    assert_has_error(&errors, "streaming.contract_path", "empty");
}

#[test]
fn live_local_non_local_contract_path_rejected_before_render() {
    let toml = replace(
        &valid_toml(),
        "[secrets]",
        "[streaming]\ncatalog_path = \"var/catalog\"\ncontract_path = \"s3://bucket/contracts/polymarket.toml\"\n\n[secrets]",
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
        "[secrets]",
        "[streaming]\ncatalog_path = \"var/catalog\"\ncontract_path = \"contracts/polymarket.toml\"\n\n[secrets]",
    );
    let errors = errors_for(&toml);
    assert_no_errors(&errors);
}

#[test]
fn legacy_strategy_block_rejected_during_parse() {
    let toml = replace(
        &valid_toml(),
        "[secrets]",
        "[strategy]\nstrategy_id = \"STRATEGY-001\"\norder_qty = \"5\"\n\n[secrets]",
    );
    let error = parse_error_for(&toml);
    assert!(error.contains("unknown field `strategy`"), "{error}");
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
        .replace("upload_attempt_timeout_secs = 30\n", "")
        .replace("roll_max_bytes = 1048576\n", "")
        .replace("roll_max_secs = 300\n", "")
        .replace("max_local_backlog_bytes = 10485760\n", "");
    let errors = errors_for(&toml);
    assert_has_error(&errors, "audit", "missing_audit");
}

#[test]
fn strategies_require_rulesets_in_live_local_input() {
    let toml = format!(
        "{}\n{}",
        valid_toml(),
        r#"
[[strategies]]
type = "eth_chainlink_taker"
[strategies.config]
strategy_id = "ETHCHAINLINKTAKER-001"
client_id = "POLYMARKET"
warmup_tick_count = 20
period_duration_secs = 300
reentry_cooldown_secs = 30
max_position_usdc = 1000.0
book_impact_cap_bps = 15
risk_lambda = 0.5
worst_case_ev_min_bps = -20
exit_hysteresis_bps = 5
vol_window_secs = 60
vol_gap_reset_secs = 10
vol_min_observations = 20
vol_bridge_valid_secs = 10
pricing_kurtosis = 0.0
theta_decay_factor = 0.0
forced_flat_stale_chainlink_ms = 1500
forced_flat_thin_book_min_liquidity = 100.0
lead_agreement_min_corr = 0.8
lead_jitter_max_ms = 250
"#
    );
    let errors = errors_for(&toml);
    assert_has_error(&errors, "strategies", "requires_rulesets");
}

#[test]
fn strategies_config_must_be_table_in_live_local_input() {
    let toml = format!(
        "{}\n{}",
        valid_phase1_toml(),
        r#"
[[strategies]]
type = "eth_chainlink_taker"
config = "oops"
"#
    );
    let errors = errors_for(&toml);
    assert_has_error(&errors, "strategies[0].config", "wrong_type");
}

#[test]
fn strategies_missing_required_builder_fields_fail_at_live_local_layer() {
    let toml = format!(
        "{}\n{}",
        valid_phase1_toml(),
        r#"
[[strategies]]
type = "eth_chainlink_taker"
[strategies.config]
strategy_id = "ETHCHAINLINKTAKER-001"
client_id = "POLYMARKET"
"#
    );
    let errors = errors_for(&toml);
    assert_has_error(
        &errors,
        "strategies[0].config.warmup_tick_count",
        "missing_warmup_tick_count",
    );
    assert_has_error(
        &errors,
        "strategies[0].config.period_duration_secs",
        "missing_period_duration_secs",
    );
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
resolution_basis = "binance_btcusdt_5m"
min_time_to_expiry_secs = 60
max_time_to_expiry_secs = 900
min_liquidity_num = 1000
require_accepting_orders = true
freeze_before_end_secs = 90
selector_poll_interval_ms = 1000
candidate_load_timeout_secs = 30
[rulesets.selector]
tag_slug = "bitcoin-2"
"#
    );
    let errors = errors_for(&toml);
    assert_has_error(&errors, "rulesets", "duplicate_ruleset_id");
}

#[test]
fn phase1_reference_rejected_when_rulesets_are_missing() {
    let toml = without_phase1_audit(&without_phase1_rulesets(&valid_phase1_toml()));
    let errors = errors_for(&toml);
    assert_has_error(&errors, "reference", "orphaned_phase1_reference");
}

#[test]
fn phase1_reference_min_publish_interval_only_rejected_when_rulesets_are_missing() {
    let toml = without_phase1_audit(&without_phase1_reference_venues(&without_phase1_rulesets(
        &valid_phase1_toml(),
    )))
    .replace("publish_topic = \"platform.reference.default\"\n", "")
    .replace(
        "min_publish_interval_ms = 100",
        "min_publish_interval_ms = 250",
    );
    let errors = errors_for(&toml);
    assert_has_error(&errors, "reference", "orphaned_phase1_reference");
}

#[test]
fn phase1_reference_zero_min_publish_interval_only_rejected_when_rulesets_are_missing() {
    let toml = without_phase1_audit(&without_phase1_reference_venues(&without_phase1_rulesets(
        &valid_phase1_toml(),
    )))
    .replace("publish_topic = \"platform.reference.default\"\n", "")
    .replace(
        "min_publish_interval_ms = 100",
        "min_publish_interval_ms = 0",
    );
    let errors = errors_for(&toml);
    assert_has_error(&errors, "reference", "orphaned_phase1_reference");
}

#[test]
fn phase1_live_local_orphaned_reference_binance_reports_only_top_level_error_without_rulesets() {
    let toml = without_phase1_audit(&without_phase1_reference_venues(&without_phase1_rulesets(
        &valid_phase1_toml(),
    )));
    let errors = errors_for(&toml);
    assert_has_error(&errors, "reference", "orphaned_phase1_reference");
    assert_lacks_error(&errors, "reference.binance", "orphaned_binance_config");
}

#[test]
fn phase1_audit_rejected_when_rulesets_are_missing() {
    let toml = valid_phase1_toml()
        .replace("[[rulesets]]\n", "")
        .replace("id = \"PRIMARY\"\n", "")
        .replace("venue = \"polymarket\"\n", "")
        .replace("[rulesets.selector]\n", "")
        .replace("tag_slug = \"bitcoin\"\n", "")
        .replace("resolution_basis = \"binance_btcusdt_1m\"\n", "")
        .replace("min_time_to_expiry_secs = 60\n", "")
        .replace("max_time_to_expiry_secs = 900\n", "")
        .replace("min_liquidity_num = 1000\n", "")
        .replace("require_accepting_orders = true\n", "")
        .replace("freeze_before_end_secs = 90\n", "")
        .replace("selector_poll_interval_ms = 1000\n", "")
        .replace("candidate_load_timeout_secs = 30\n", "")
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
    let errors = errors_for(&toml);
    assert_has_error(&errors, "audit", "orphaned_phase1_audit");
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
fn phase1_reference_publish_topic_must_be_non_empty_when_rulesets_are_configured() {
    let toml = replace(
        &valid_phase1_toml(),
        "publish_topic = \"platform.reference.default\"",
        "publish_topic = \"\"",
    );
    let errors = errors_for(&toml);
    assert_has_error(&errors, "reference.publish_topic", "empty");
}

#[test]
fn phase1_reference_min_publish_interval_ms_must_be_positive_when_rulesets_are_configured() {
    let toml = replace(
        &valid_phase1_toml(),
        "min_publish_interval_ms = 100",
        "min_publish_interval_ms = 0",
    );
    let errors = errors_for(&toml);
    assert_has_error(&errors, "reference.min_publish_interval_ms", "not_positive");
}

#[test]
fn phase1_reference_venue_instrument_id_must_be_non_empty() {
    let toml = replace(
        &valid_phase1_toml(),
        "instrument_id = \"BTCUSDT.BINANCE\"",
        "instrument_id = \"\"",
    );
    let errors = errors_for(&toml);
    assert_has_error(&errors, "reference.venues[0].instrument_id", "empty");
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

#[test]
fn phase1_ruleset_selector_tag_slug_must_be_non_empty() {
    let toml = replace(
        &valid_phase1_toml(),
        "tag_slug = \"bitcoin\"",
        "tag_slug = \"\"",
    );
    let errors = errors_for(&toml);
    assert_has_error(&errors, "rulesets[0].selector.tag_slug", "empty");
}

#[test]
fn phase1_ruleset_selector_tag_slug_must_not_contain_whitespace() {
    let toml = replace(
        &valid_phase1_toml(),
        "tag_slug = \"bitcoin\"",
        "tag_slug = \" bitcoin \"",
    );
    let errors = errors_for(&toml);
    assert_has_error(
        &errors,
        "rulesets[0].selector.tag_slug",
        "contains_whitespace",
    );
}

#[test]
fn phase1_ruleset_selector_unknown_field_rejected() {
    let toml = valid_phase1_toml().replace(
        "[rulesets.selector]\ntag_slug = \"bitcoin\"",
        "[rulesets.selector]\ntag_slug = \"bitcoin\"\nevent_slug_prefx = \"btc-5m\"",
    );
    let errors = errors_for(&toml);
    assert_has_error(
        &errors,
        "rulesets[0].selector.event_slug_prefx",
        "unknown_field",
    );
}

#[test]
fn phase1_event_slug_rejected_when_rulesets_are_enabled() {
    let toml = replace(
        &valid_phase1_toml(),
        "[polymarket]\n",
        "[polymarket]\nevent_slug = \"btc-updown-5m\"\n",
    );
    let errors = errors_for(&toml);
    assert_has_error(
        &errors,
        "polymarket.event_slug",
        "forbidden_in_ruleset_mode",
    );
}

#[test]
fn phase1_ruleset_resolution_basis_must_be_non_empty() {
    let toml = replace(
        &valid_phase1_toml(),
        "resolution_basis = \"binance_btcusdt_1m\"",
        "resolution_basis = \"\"",
    );
    let errors = errors_for(&toml);
    assert_has_error(&errors, "rulesets[0].resolution_basis", "empty");
}

#[test]
fn phase1_ruleset_resolution_basis_must_be_canonical() {
    let toml = replace(
        &valid_phase1_toml(),
        "resolution_basis = \"binance_btcusdt_1m\"",
        "resolution_basis = \"Binance_BTCUSDT_1m\"",
    );
    let errors = errors_for(&toml);
    assert_has_error(
        &errors,
        "rulesets[0].resolution_basis",
        "invalid_resolution_basis",
    );
}

#[test]
fn phase1_ruleset_min_time_to_expiry_secs_must_be_positive() {
    let toml = replace(
        &valid_phase1_toml(),
        "min_time_to_expiry_secs = 60",
        "min_time_to_expiry_secs = 0",
    );
    let errors = errors_for(&toml);
    assert_has_error(
        &errors,
        "rulesets[0].min_time_to_expiry_secs",
        "not_positive",
    );
}

#[test]
fn phase1_ruleset_max_time_to_expiry_secs_must_be_positive() {
    let toml = replace(
        &valid_phase1_toml(),
        "max_time_to_expiry_secs = 900",
        "max_time_to_expiry_secs = 0",
    );
    let errors = errors_for(&toml);
    assert_has_error(
        &errors,
        "rulesets[0].max_time_to_expiry_secs",
        "not_positive",
    );
}

#[test]
fn phase1_ruleset_max_time_to_expiry_secs_must_not_be_less_than_min_time_to_expiry_secs() {
    let toml = replace(
        &valid_phase1_toml(),
        "max_time_to_expiry_secs = 900",
        "max_time_to_expiry_secs = 30",
    );
    let errors = errors_for(&toml);
    assert_has_error(
        &errors,
        "rulesets[0].max_time_to_expiry_secs",
        "invalid_max_time_to_expiry_secs",
    );
}

#[test]
fn phase1_ruleset_freeze_before_end_secs_must_not_precede_min_time_to_expiry_secs() {
    let toml = replace(
        &valid_phase1_toml(),
        "freeze_before_end_secs = 90",
        "freeze_before_end_secs = 30",
    );
    let errors = errors_for(&toml);
    assert_has_error(
        &errors,
        "rulesets[0].freeze_before_end_secs",
        "invalid_freeze_before_end_secs",
    );
}

#[test]
fn phase1_ruleset_min_liquidity_num_must_be_non_negative_and_finite() {
    let negative = replace(
        &valid_phase1_toml(),
        "min_liquidity_num = 1000",
        "min_liquidity_num = -1.0",
    );
    let negative_errors = errors_for(&negative);
    assert_has_error(
        &negative_errors,
        "rulesets[0].min_liquidity_num",
        "not_non_negative_finite",
    );

    let nan = replace(
        &valid_phase1_toml(),
        "min_liquidity_num = 1000",
        "min_liquidity_num = nan",
    );
    let nan_errors = errors_for(&nan);
    assert_has_error(
        &nan_errors,
        "rulesets[0].min_liquidity_num",
        "not_non_negative_finite",
    );
}

#[test]
fn phase1_ruleset_selector_poll_interval_ms_must_be_positive() {
    let toml = replace(
        &valid_phase1_toml(),
        "selector_poll_interval_ms = 1000",
        "selector_poll_interval_ms = 0",
    );
    let errors = errors_for(&toml);
    assert_has_error(
        &errors,
        "rulesets[0].selector_poll_interval_ms",
        "not_positive",
    );
}

#[test]
fn phase1_ruleset_candidate_load_timeout_secs_must_be_positive() {
    let toml = replace(
        &valid_phase1_toml(),
        "candidate_load_timeout_secs = 30",
        "candidate_load_timeout_secs = 0",
    );
    let errors = errors_for(&toml);
    assert_has_error(
        &errors,
        "rulesets[0].candidate_load_timeout_secs",
        "not_positive",
    );
}

#[test]
fn phase1_audit_paths_must_be_non_empty() {
    let local_dir = replace(
        &valid_phase1_toml(),
        "local_dir = \"/srv/bolt-v2/var/audit\"",
        "local_dir = \"\"",
    );
    let local_dir_errors = errors_for(&local_dir);
    assert_has_error(&local_dir_errors, "audit.local_dir", "empty");

    let s3_uri = replace(
        &valid_phase1_toml(),
        "s3_uri = \"s3://bolt-runtime-history/phase1\"",
        "s3_uri = \"\"",
    );
    let s3_uri_errors = errors_for(&s3_uri);
    assert_has_error(&s3_uri_errors, "audit.s3_uri", "empty");
}

#[test]
fn live_local_runtime_write_dirs_must_be_absolute() {
    let toml = valid_phase1_toml()
        .replace("/srv/bolt-v2/var/raw", "var/raw")
        .replace("/srv/bolt-v2/var/audit", "var/audit");
    let errors = errors_for(&toml);
    assert_has_error(&errors, "raw_capture.output_dir", "not_absolute");
    assert_has_error(&errors, "audit.local_dir", "not_absolute");
}

#[test]
fn live_local_raw_capture_output_dir_must_be_non_empty() {
    let toml = replace(
        &valid_toml(),
        "output_dir = \"/srv/bolt-v2/var/raw\"",
        "output_dir = \"\"",
    );
    let errors = errors_for(&toml);
    assert_has_error(&errors, "raw_capture.output_dir", "empty");
}

#[test]
fn phase1_audit_intervals_and_limits_must_be_positive() {
    let ship_interval = replace(
        &valid_phase1_toml(),
        "ship_interval_secs = 30",
        "ship_interval_secs = 0",
    );
    let ship_interval_errors = errors_for(&ship_interval);
    assert_has_error(
        &ship_interval_errors,
        "audit.ship_interval_secs",
        "not_positive",
    );

    let upload_attempt_timeout = replace(
        &valid_phase1_toml(),
        "upload_attempt_timeout_secs = 30",
        "upload_attempt_timeout_secs = 0",
    );
    let upload_attempt_timeout_errors = errors_for(&upload_attempt_timeout);
    assert_has_error(
        &upload_attempt_timeout_errors,
        "audit.upload_attempt_timeout_secs",
        "not_positive",
    );

    let roll_max_bytes = replace(
        &valid_phase1_toml(),
        "roll_max_bytes = 1048576",
        "roll_max_bytes = 0",
    );
    let roll_max_bytes_errors = errors_for(&roll_max_bytes);
    assert_has_error(
        &roll_max_bytes_errors,
        "audit.roll_max_bytes",
        "not_positive",
    );

    let roll_max_secs = replace(
        &valid_phase1_toml(),
        "roll_max_secs = 300",
        "roll_max_secs = 0",
    );
    let roll_max_secs_errors = errors_for(&roll_max_secs);
    assert_has_error(&roll_max_secs_errors, "audit.roll_max_secs", "not_positive");

    let backlog = replace(
        &valid_phase1_toml(),
        "max_local_backlog_bytes = 10485760",
        "max_local_backlog_bytes = 0",
    );
    let backlog_errors = errors_for(&backlog);
    assert_has_error(
        &backlog_errors,
        "audit.max_local_backlog_bytes",
        "not_positive",
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

[raw_capture]
output_dir = "/srv/bolt-v2/var/raw"
"#
}

fn valid_runtime_toml_with_stub_strategy() -> String {
    format!(
        "{}\n{}",
        valid_runtime_toml(),
        r#"
[[strategies]]
type = "stub_runtime_strategy"
[strategies.config]
strategy_id = "STUB-RUNTIME-001"
instrument_id = "0xabc-12345.POLYMARKET"
client_id = "POLYMARKET"
order_qty = "5"
"#
    )
}

fn valid_phase1_toml() -> String {
    format!(
        "{}\n{}\n{}",
        valid_toml().replace("event_slug = \"btc-updown-5m\"\n", ""),
        VALID_BINANCE_SHARED_BLOCK,
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
resolution_basis = "binance_btcusdt_1m"
min_time_to_expiry_secs = 60
max_time_to_expiry_secs = 900
min_liquidity_num = 1000
require_accepting_orders = true
freeze_before_end_secs = 90
selector_poll_interval_ms = 1000
candidate_load_timeout_secs = 30
[rulesets.selector]
tag_slug = "bitcoin"

[audit]
local_dir = "/srv/bolt-v2/var/audit"
s3_uri = "s3://bolt-runtime-history/phase1"
ship_interval_secs = 30
upload_attempt_timeout_secs = 30
roll_max_bytes = 1048576
roll_max_secs = 300
max_local_backlog_bytes = 10485760
"#
    )
}

const PHASE1_REFERENCE_BINANCE_VENUE_BLOCK: &str = r#"[[reference.venues]]
name = "BINANCE-BTC"
type = "binance"
instrument_id = "BTCUSDT.BINANCE"
base_weight = 0.35
stale_after_ms = 1500
disable_after_ms = 5000
"#;

const PHASE1_RULESET_BLOCK: &str = r#"[[rulesets]]
id = "PRIMARY"
venue = "polymarket"
resolution_basis = "binance_btcusdt_1m"
min_time_to_expiry_secs = 60
max_time_to_expiry_secs = 900
min_liquidity_num = 1000
require_accepting_orders = true
freeze_before_end_secs = 90
selector_poll_interval_ms = 1000
candidate_load_timeout_secs = 30
[rulesets.selector]
tag_slug = "bitcoin"
"#;

const PHASE1_AUDIT_BLOCK: &str = r#"[audit]
local_dir = "/srv/bolt-v2/var/audit"
s3_uri = "s3://bolt-runtime-history/phase1"
ship_interval_secs = 30
upload_attempt_timeout_secs = 30
roll_max_bytes = 1048576
roll_max_secs = 300
max_local_backlog_bytes = 10485760
"#;

fn without_phase1_rulesets(base: &str) -> String {
    strip_block(base, PHASE1_RULESET_BLOCK)
}

fn without_phase1_reference_venues(base: &str) -> String {
    strip_block(base, PHASE1_REFERENCE_BINANCE_VENUE_BLOCK)
}

fn without_phase1_audit(base: &str) -> String {
    strip_block(base, PHASE1_AUDIT_BLOCK)
}

fn valid_phase1_runtime_toml() -> String {
    format!(
        "{VALID_BINANCE_SHARED_BLOCK}\n{}\n{}",
        valid_runtime_toml().replace(
            "event_slugs = [\"btc-updown-5m\"]\n",
            "event_slugs = [\"btc-updown-5m\"]\ngamma_event_fetch_max_concurrent = 8\n",
        ),
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
resolution_basis = "binance_btcusdt_1m"
min_time_to_expiry_secs = 60
max_time_to_expiry_secs = 900
min_liquidity_num = 1000
require_accepting_orders = true
freeze_before_end_secs = 90
selector_poll_interval_ms = 1000
candidate_load_timeout_secs = 30
[rulesets.selector]
tag_slug = "bitcoin"

[audit]
local_dir = "/srv/bolt-v2/var/audit"
s3_uri = "s3://bolt-runtime-history/phase1"
ship_interval_secs = 30
upload_attempt_timeout_secs = 30
roll_max_bytes = 1048576
roll_max_secs = 300
max_local_backlog_bytes = 10485760
"#
    )
}

const VALID_CHAINLINK_SHARED_BLOCK: &str = r#"[reference.chainlink]
region = "us-east-1"
api_key = "/bolt/chainlink/api_key"
api_secret = "/bolt/chainlink/api_secret"
ws_url = "wss://streams.chain.link"
ws_reconnect_alert_threshold = 5
"#;

const VALID_BINANCE_SHARED_BLOCK: &str = r#"[reference.binance]
region = "eu-west-1"
api_key = "/bolt/binance/api-key"
api_secret = "/bolt/binance/api-secret"
environment = "Mainnet"
product_types = ["SPOT"]
instrument_status_poll_secs = 3600
"#;

struct StubRuntimeTemplateBuilder;

impl StrategyBuilder for StubRuntimeTemplateBuilder {
    fn kind() -> &'static str {
        "stub_runtime_strategy"
    }

    fn validate_config(
        _raw: &toml::Value,
        _field_prefix: &str,
        _errors: &mut Vec<ValidationError>,
    ) {
    }

    fn build(_raw: &toml::Value, _context: &StrategyBuildContext) -> anyhow::Result<BoxedStrategy> {
        Err(anyhow!(
            "validate tests should not build runtime strategies"
        ))
    }

    fn register(
        _raw: &toml::Value,
        _context: &StrategyBuildContext,
        _trader: &Rc<RefCell<Trader>>,
    ) -> anyhow::Result<StrategyId> {
        Err(anyhow!(
            "validate tests should not register runtime strategies"
        ))
    }
}

fn stub_runtime_registry() -> StrategyRegistry {
    let mut registry = StrategyRegistry::new();
    registry
        .register::<StubRuntimeTemplateBuilder>()
        .expect("stub runtime builder should register");
    registry
}

fn runtime_errors_for(toml_str: &str) -> Vec<ValidationError> {
    let config: Config = toml::from_str(toml_str).expect("runtime test config should parse");
    validate_runtime(&config)
}

fn runtime_errors_for_with_registry(
    toml_str: &str,
    registry: &StrategyRegistry,
) -> Vec<ValidationError> {
    let config: Config = toml::from_str(toml_str).expect("runtime test config should parse");
    validate_runtime_with_registry(&config, registry)
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
fn runtime_missing_event_slugs_allowed_when_rulesets_drive_selection() {
    let toml = valid_phase1_runtime_toml().replace("event_slugs = [\"btc-updown-5m\"]\n", "");
    let errors = runtime_errors_for(&toml);
    assert_no_errors(&errors);
}

#[test]
fn runtime_legacy_event_slugs_rejected_when_rulesets_drive_selection() {
    let toml = valid_phase1_runtime_toml();
    let errors = runtime_errors_for(&toml);
    assert_has_error(
        &errors,
        "data_clients[0].config.event_slugs",
        "forbidden_in_ruleset_mode",
    );
}

#[test]
fn runtime_malformed_legacy_event_slugs_rejected_when_rulesets_drive_selection() {
    let toml =
        valid_phase1_runtime_toml().replace("event_slugs = [\"btc-updown-5m\"]", "event_slugs = 7");
    let errors = runtime_errors_for(&toml);
    assert_has_error(
        &errors,
        "data_clients[0].config.event_slugs",
        "forbidden_in_ruleset_mode",
    );
}

#[test]
fn runtime_gamma_refresh_interval_secs_must_be_positive_when_present() {
    let toml = valid_runtime_toml().replace(
        "event_slugs = [\"btc-updown-5m\"]",
        "event_slugs = [\"btc-updown-5m\"]\ngamma_refresh_interval_secs = 0",
    );
    let errors = runtime_errors_for(&toml);
    assert_has_error(
        &errors,
        "data_clients[0].config.gamma_refresh_interval_secs",
        "not_positive",
    );
}

#[test]
fn runtime_gamma_refresh_interval_secs_wrong_type_rejected_when_present() {
    let toml = valid_runtime_toml().replace(
        "event_slugs = [\"btc-updown-5m\"]",
        "event_slugs = [\"btc-updown-5m\"]\ngamma_refresh_interval_secs = \"fast\"",
    );
    let errors = runtime_errors_for(&toml);
    assert_has_error(
        &errors,
        "data_clients[0].config.gamma_refresh_interval_secs",
        "wrong_type",
    );
}

#[test]
fn runtime_gamma_event_fetch_max_concurrent_must_be_positive_when_present() {
    let toml = valid_runtime_toml().replace(
        "event_slugs = [\"btc-updown-5m\"]",
        "event_slugs = [\"btc-updown-5m\"]\ngamma_event_fetch_max_concurrent = 0",
    );
    let errors = runtime_errors_for(&toml);
    assert_has_error(
        &errors,
        "data_clients[0].config.gamma_event_fetch_max_concurrent",
        "not_positive",
    );
}

#[test]
fn runtime_gamma_event_fetch_max_concurrent_wrong_type_rejected_when_present() {
    let toml = valid_runtime_toml().replace(
        "event_slugs = [\"btc-updown-5m\"]",
        "event_slugs = [\"btc-updown-5m\"]\ngamma_event_fetch_max_concurrent = \"wide\"",
    );
    let errors = runtime_errors_for(&toml);
    assert_has_error(
        &errors,
        "data_clients[0].config.gamma_event_fetch_max_concurrent",
        "wrong_type",
    );
}

#[test]
fn runtime_gamma_event_fetch_max_concurrent_required_when_rulesets_are_enabled() {
    let toml = valid_phase1_runtime_toml()
        .replace("event_slugs = [\"btc-updown-5m\"]\n", "")
        .replace("gamma_event_fetch_max_concurrent = 8\n", "");
    let errors = runtime_errors_for(&toml);
    assert_has_error(
        &errors,
        "data_clients[0].config.gamma_event_fetch_max_concurrent",
        "missing_gamma_event_fetch_max_concurrent",
    );
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
        valid_runtime_toml_with_stub_strategy(),
        r#"
[[strategies]]
type = "stub_runtime_strategy"
[strategies.config]
strategy_id = "STUB-RUNTIME-001"
instrument_id = "0xdef-67890.POLYMARKET"
client_id = "POLYMARKET"
order_qty = "10"
"#,
        r#"
[[strategies]]
type = "stub_runtime_strategy"
[strategies.config]
strategy_id = "STUB-RUNTIME-001"
instrument_id = "0xghi-13579.POLYMARKET"
client_id = "POLYMARKET"
order_qty = "15"
"#
    );
    let registry = stub_runtime_registry();
    let errors = runtime_errors_for_with_registry(&toml, &registry);
    assert_has_error(&errors, "strategies", "duplicate_strategy_id");
    assert_error_message_contains(
        &errors,
        "strategies",
        "duplicate_strategy_id",
        "strategies[1] has duplicate strategy_id \"STUB-RUNTIME-001\"",
    );
    assert!(
        errors.iter().any(|e| {
            e.field == "strategies"
                && e.code == "duplicate_strategy_id"
                && e.message.contains(
                    "strategies[2] has duplicate strategy_id \"STUB-RUNTIME-001\" (first defined at strategies[0])"
                )
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
    let toml = valid_runtime_toml_with_stub_strategy()
        .replace("client_id = \"POLYMARKET\"", "client_id = \"NONEXISTENT\"");
    let registry = stub_runtime_registry();
    let errors = runtime_errors_for_with_registry(&toml, &registry);
    assert_has_error(&errors, "strategies", "unknown_client_id");
}

#[test]
fn strategy_referencing_existing_client_accepted() {
    let registry = stub_runtime_registry();
    let errors =
        runtime_errors_for_with_registry(&valid_runtime_toml_with_stub_strategy(), &registry);
    assert!(
        !errors.iter().any(|e| e.code == "unknown_client_id"),
        "valid client_id should not produce errors"
    );
}

#[test]
fn runtime_missing_strategy_id_rejected() {
    let toml =
        valid_runtime_toml_with_stub_strategy().replace("strategy_id = \"STUB-RUNTIME-001\"\n", "");
    let registry = stub_runtime_registry();
    let errors = runtime_errors_for_with_registry(&toml, &registry);
    assert_has_error(
        &errors,
        "strategies[0].config.strategy_id",
        "missing_strategy_id",
    );
}

#[test]
fn runtime_strategy_id_wrong_type_rejected() {
    let toml = valid_runtime_toml_with_stub_strategy()
        .replace("strategy_id = \"STUB-RUNTIME-001\"", "strategy_id = 42");
    let registry = stub_runtime_registry();
    let errors = runtime_errors_for_with_registry(&toml, &registry);
    assert_has_error(&errors, "strategies[0].config.strategy_id", "wrong_type");
    assert_error_message_contains(
        &errors,
        "strategies[0].config.strategy_id",
        "wrong_type",
        "must be a string, got integer value",
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
fn runtime_relative_runtime_write_dirs_rejected() {
    let toml = valid_phase1_runtime_toml()
        .replace("event_slugs = [\"btc-updown-5m\"]\n", "")
        .replace("/srv/bolt-v2/var/raw", "var/raw")
        .replace("/srv/bolt-v2/var/audit", "var/audit");
    let errors = runtime_errors_for(&toml);
    assert_has_error(&errors, "raw_capture.output_dir", "not_absolute");
    assert_has_error(&errors, "audit.local_dir", "not_absolute");
}

#[test]
fn runtime_raw_capture_output_dir_must_be_non_empty() {
    let toml =
        valid_runtime_toml().replace("output_dir = \"/srv/bolt-v2/var/raw\"", "output_dir = \"\"");
    let errors = runtime_errors_for(&toml);
    assert_has_error(&errors, "raw_capture.output_dir", "empty");
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
        "local absolute path",
    );
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
        .replacen("type = \"polymarket\"", "type = \"bogus\"", 1);
    let errors = runtime_errors_for(&toml);
    assert_has_error(&errors, "data_clients[0].type", "unsupported_type");
    assert_has_error(&errors, "exec_clients[0].type", "unsupported_type");
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
resolution_basis = "binance_btcusdt_5m"
min_time_to_expiry_secs = 60
max_time_to_expiry_secs = 900
min_liquidity_num = 1000
require_accepting_orders = true
freeze_before_end_secs = 90
selector_poll_interval_ms = 1000
candidate_load_timeout_secs = 30
[rulesets.selector]
tag_slug = "bitcoin-2"
"#
    );
    let errors = runtime_errors_for(&toml);
    assert_has_error(&errors, "rulesets", "phase1_single_active_ruleset");
}

#[test]
fn phase1_runtime_requires_reference_venues_when_one_ruleset_is_active() {
    let toml = strip_block(&valid_phase1_runtime_toml(), VALID_BINANCE_SHARED_BLOCK)
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
fn phase1_runtime_requires_audit_when_ruleset_is_configured() {
    let toml = valid_phase1_runtime_toml()
        .replace("[audit]\n", "")
        .replace("local_dir = \"var/audit\"\n", "")
        .replace("s3_uri = \"s3://bolt-runtime-history/phase1\"\n", "")
        .replace("ship_interval_secs = 30\n", "")
        .replace("upload_attempt_timeout_secs = 30\n", "")
        .replace("roll_max_bytes = 1048576\n", "")
        .replace("roll_max_secs = 300\n", "")
        .replace("max_local_backlog_bytes = 10485760\n", "");
    let errors = runtime_errors_for(&toml);
    assert_has_error(&errors, "audit", "missing_audit");
}

#[test]
fn phase1_runtime_allows_zero_runtime_templates() {
    let toml = valid_phase1_runtime_toml().replace("event_slugs = [\"btc-updown-5m\"]\n", "");
    let errors = runtime_errors_for(&toml);
    assert_no_errors(&errors);
}

#[test]
fn phase1_runtime_load_accepts_zero_runtime_templates() {
    let toml = valid_phase1_runtime_toml().replace("event_slugs = [\"btc-updown-5m\"]\n", "");

    let mut file = NamedTempFile::new().expect("runtime temp file should be created");
    file.write_all(toml.as_bytes())
        .expect("runtime temp file should be written");
    Config::load(file.path()).expect("zero-template ruleset config should load");
}

#[test]
fn phase1_runtime_load_rejects_missing_gamma_event_fetch_max_concurrent() {
    let toml = valid_phase1_runtime_toml()
        .replace("event_slugs = [\"btc-updown-5m\"]\n", "")
        .replace("gamma_event_fetch_max_concurrent = 8\n", "");

    let error = runtime_load_error_for(&toml);
    assert!(
        error.contains("data_clients[0].config.gamma_event_fetch_max_concurrent"),
        "runtime load error should mention gamma_event_fetch_max_concurrent: {error}"
    );
}

#[test]
fn phase1_runtime_accepts_registered_stub_runtime_template_via_registry() {
    let toml = format!(
        "{}\n{}",
        valid_phase1_runtime_toml().replace("event_slugs = [\"btc-updown-5m\"]\n", ""),
        r#"
[[strategies]]
type = "stub_runtime_strategy"
[strategies.config]
strategy_id = "STUB-RUNTIME-001"
"#
    );
    let registry = stub_runtime_registry();
    let errors = runtime_errors_for_with_registry(&toml, &registry);

    assert!(
        !errors
            .iter()
            .any(|e| e.field == "strategies[0].type" && e.code == "unsupported_type"),
        "registered stub runtime kind should be accepted, got: {errors:?}"
    );
    assert!(
        !errors
            .iter()
            .any(|e| e.field == "strategies" && e.code == "phase1_runtime_strategy_template_count"),
        "registered stub runtime kind should satisfy the runtime template invariant, got: {errors:?}"
    );
    assert!(
        !errors
            .iter()
            .any(|e| e.field == "strategies[0].config.client_id" && e.code == "missing_client_id"),
        "stub runtime validation should not inherit removed template-only client_id requirements, got: {errors:?}"
    );
    assert!(
        !errors
            .iter()
            .any(|e| e.field == "strategies[0].config.instrument_id"
                && e.code == "missing_instrument_id"),
        "stub runtime validation should not inherit removed template-only instrument_id requirements, got: {errors:?}"
    );
    assert!(
        !errors
            .iter()
            .any(|e| e.field == "strategies[0].config.order_qty" && e.code == "missing_order_qty"),
        "stub runtime validation should not inherit removed template-only order_qty requirements, got: {errors:?}"
    );
}

#[test]
fn phase1_runtime_rejects_duplicate_runtime_templates() {
    let toml = format!(
        "{}\n{}\n{}",
        valid_phase1_runtime_toml().replace("event_slugs = [\"btc-updown-5m\"]\n", ""),
        r#"
[[strategies]]
type = "stub_runtime_strategy"
[strategies.config]
strategy_id = "STUB-RUNTIME-002"
"#,
        r#"
[[strategies]]
type = "stub_runtime_strategy"
[strategies.config]
strategy_id = "STUB-RUNTIME-003"
"#
    );
    let registry = stub_runtime_registry();
    let errors = runtime_errors_for_with_registry(&toml, &registry);
    assert_has_error(
        &errors,
        "strategies",
        "phase1_runtime_strategy_template_count",
    );
    assert_error_message_contains(
        &errors,
        "strategies",
        "phase1_runtime_strategy_template_count",
        "at most one runtime strategy template",
    );
}

#[test]
fn phase1_runtime_load_rejects_unsupported_runtime_template_kind() {
    let toml = format!(
        "{}\n{}",
        valid_phase1_runtime_toml(),
        r#"
[[strategies]]
type = "bogus"
[strategies.config]
strategy_id = "STUB-RUNTIME-002"
"#
    );
    let error = runtime_load_error_for(&toml);
    assert!(
        error.contains("strategies[0].type"),
        "runtime load error should mention unsupported strategy type: {error}"
    );
}

#[test]
fn phase1_runtime_load_rejects_missing_audit_when_ruleset_is_configured() {
    let toml = valid_phase1_runtime_toml()
        .replace("[audit]\n", "")
        .replace("local_dir = \"var/audit\"\n", "")
        .replace("s3_uri = \"s3://bolt-runtime-history/phase1\"\n", "")
        .replace("ship_interval_secs = 30\n", "")
        .replace("upload_attempt_timeout_secs = 30\n", "")
        .replace("roll_max_bytes = 1048576\n", "")
        .replace("roll_max_secs = 300\n", "")
        .replace("max_local_backlog_bytes = 10485760\n", "");
    let error = runtime_load_error_for(&toml);
    assert!(
        error.contains("audit"),
        "runtime load error should mention audit: {error}"
    );
}

#[test]
fn phase1_runtime_rejects_empty_ruleset_id() {
    let toml = replace(
        &valid_phase1_runtime_toml(),
        "id = \"PRIMARY\"",
        "id = \"\"",
    );
    let errors = runtime_errors_for(&toml);
    assert_has_error(&errors, "rulesets[0].id", "empty");
}

#[test]
fn phase1_runtime_rejects_duplicate_ruleset_ids() {
    let toml = format!(
        "{}\n{}",
        valid_phase1_runtime_toml(),
        r#"
[[rulesets]]
id = "PRIMARY"
venue = "polymarket"
resolution_basis = "binance_btcusdt_5m"
min_time_to_expiry_secs = 60
max_time_to_expiry_secs = 900
min_liquidity_num = 1000
require_accepting_orders = true
freeze_before_end_secs = 90
selector_poll_interval_ms = 1000
candidate_load_timeout_secs = 30
[rulesets.selector]
tag_slug = "bitcoin-2"
"#
    );
    let errors = runtime_errors_for(&toml);
    assert_has_error(&errors, "rulesets", "duplicate_ruleset_id");
}

#[test]
fn phase1_runtime_load_rejects_duplicate_ruleset_ids() {
    let toml = format!(
        "{}\n{}",
        valid_phase1_runtime_toml(),
        r#"
[[rulesets]]
id = "PRIMARY"
venue = "polymarket"
resolution_basis = "binance_btcusdt_5m"
min_time_to_expiry_secs = 60
max_time_to_expiry_secs = 900
min_liquidity_num = 1000
require_accepting_orders = true
freeze_before_end_secs = 90
selector_poll_interval_ms = 1000
candidate_load_timeout_secs = 30
[rulesets.selector]
tag_slug = "bitcoin-2"
"#
    );
    let error = runtime_load_error_for(&toml);
    assert!(
        error.contains("duplicate id"),
        "runtime load error should mention duplicate ruleset id: {error}"
    );
}

#[test]
fn phase1_runtime_rejects_orphaned_reference_min_publish_interval_without_rulesets() {
    let toml = without_phase1_audit(&without_phase1_reference_venues(&without_phase1_rulesets(
        &valid_phase1_runtime_toml(),
    )))
    .replace(
        "publish_topic = \"platform.reference.default\"",
        "publish_topic = \"\"",
    )
    .replace(
        "min_publish_interval_ms = 100",
        "min_publish_interval_ms = 250",
    );
    let errors = runtime_errors_for(&toml);
    assert_has_error(&errors, "reference", "orphaned_phase1_reference");
}

#[test]
fn phase1_runtime_rejects_orphaned_reference_zero_min_publish_interval_without_rulesets() {
    let toml = without_phase1_audit(&without_phase1_reference_venues(&without_phase1_rulesets(
        &valid_phase1_runtime_toml(),
    )))
    .replace(
        "publish_topic = \"platform.reference.default\"",
        "publish_topic = \"\"",
    )
    .replace(
        "min_publish_interval_ms = 100",
        "min_publish_interval_ms = 0",
    );
    let errors = runtime_errors_for(&toml);
    assert_has_error(&errors, "reference", "orphaned_phase1_reference");
}

#[test]
fn phase1_runtime_orphaned_reference_binance_reports_only_top_level_error_without_rulesets() {
    let toml = without_phase1_audit(&without_phase1_reference_venues(&without_phase1_rulesets(
        &valid_phase1_runtime_toml().replace("event_slugs = [\"btc-updown-5m\"]\n", ""),
    )));
    let errors = runtime_errors_for(&toml);
    assert_has_error(&errors, "reference", "orphaned_phase1_reference");
    assert_lacks_error(&errors, "reference.binance", "orphaned_binance_config");
}

#[test]
fn phase1_runtime_rejects_empty_reference_venue_name() {
    let toml = replace(
        &valid_phase1_runtime_toml(),
        "name = \"BINANCE-BTC\"",
        "name = \"\"",
    );
    let errors = runtime_errors_for(&toml);
    assert_has_error(&errors, "reference.venues[0].name", "empty");
}

#[test]
fn phase1_runtime_rejects_reference_venue_non_positive_weight() {
    let toml = replace(
        &valid_phase1_runtime_toml(),
        "base_weight = 0.35",
        "base_weight = 0.0",
    );
    let errors = runtime_errors_for(&toml);
    assert_has_error(
        &errors,
        "reference.venues[0].base_weight",
        "not_positive_finite",
    );
}

#[test]
fn phase1_runtime_rejects_reference_venue_non_positive_stale_after_ms() {
    let toml = replace(
        &valid_phase1_runtime_toml(),
        "stale_after_ms = 1500",
        "stale_after_ms = 0",
    );
    let errors = runtime_errors_for(&toml);
    assert_has_error(
        &errors,
        "reference.venues[0].stale_after_ms",
        "not_positive",
    );
}

#[test]
fn phase1_runtime_rejects_reference_venue_disable_after_ms_before_stale_after_ms() {
    let toml = replace(
        &valid_phase1_runtime_toml(),
        "disable_after_ms = 5000",
        "disable_after_ms = 1000",
    );
    let errors = runtime_errors_for(&toml);
    assert_has_error(
        &errors,
        "reference.venues[0].disable_after_ms",
        "invalid_disable_after_ms",
    );
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
fn phase1_runtime_rejects_freeze_before_end_secs_before_min_time_to_expiry_secs() {
    let toml = replace(
        &valid_phase1_runtime_toml(),
        "freeze_before_end_secs = 90",
        "freeze_before_end_secs = 30",
    );
    let errors = runtime_errors_for(&toml);
    assert_has_error(
        &errors,
        "rulesets[0].freeze_before_end_secs",
        "invalid_freeze_before_end_secs",
    );
}

#[test]
fn phase1_runtime_rejects_non_positive_selector_poll_interval_ms() {
    let toml = replace(
        &valid_phase1_runtime_toml(),
        "selector_poll_interval_ms = 1000",
        "selector_poll_interval_ms = 0",
    );
    let errors = runtime_errors_for(&toml);
    assert_has_error(
        &errors,
        "rulesets[0].selector_poll_interval_ms",
        "not_positive",
    );
}

#[test]
fn phase1_runtime_rejects_non_positive_candidate_load_timeout_secs() {
    let toml = replace(
        &valid_phase1_runtime_toml(),
        "candidate_load_timeout_secs = 30",
        "candidate_load_timeout_secs = 0",
    );
    let errors = runtime_errors_for(&toml);
    assert_has_error(
        &errors,
        "rulesets[0].candidate_load_timeout_secs",
        "not_positive",
    );
}

#[test]
fn phase1_runtime_rejects_non_positive_audit_upload_attempt_timeout_secs() {
    let toml = replace(
        &valid_phase1_runtime_toml(),
        "upload_attempt_timeout_secs = 30",
        "upload_attempt_timeout_secs = 0",
    );
    let errors = runtime_errors_for(&toml);
    assert_has_error(&errors, "audit.upload_attempt_timeout_secs", "not_positive");
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
fn phase1_runtime_rejects_non_canonical_resolution_basis() {
    let toml = replace(
        &valid_phase1_runtime_toml(),
        "resolution_basis = \"binance_btcusdt_1m\"",
        "resolution_basis = \"Binance_BTCUSDT_1m\"",
    );
    let errors = runtime_errors_for(&toml);
    assert_has_error(
        &errors,
        "rulesets[0].resolution_basis",
        "invalid_resolution_basis",
    );
}

#[test]
fn phase1_runtime_rejects_unknown_resolution_basis_source() {
    let toml = replace(
        &valid_phase1_runtime_toml(),
        "resolution_basis = \"binance_btcusdt_1m\"",
        "resolution_basis = \"coinbase_btcusd_1m\"",
    );
    let errors = runtime_errors_for(&toml);
    assert_has_error(
        &errors,
        "rulesets[0].resolution_basis",
        "invalid_resolution_basis",
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
    // Unified with the reference-independent invariant (PR #183 Fix 1): any
    // second polymarket data_client is rejected regardless of reference venue.
    assert_has_error(&errors, "data_clients", "duplicate_polymarket_client");
}

#[test]
fn phase1_runtime_rejects_multiple_polymarket_data_clients_without_polymarket_reference() {
    // PR #183 Fix 1: the runtime assumes a single shared PolymarketRulesetSetup,
    // so even when polymarket is NOT a reference venue, more than one
    // polymarket data_client is unsupported and must be rejected at validation.
    let toml = format!(
        "{}\n{}",
        valid_phase1_runtime_toml(),
        r#"
[[data_clients]]
name = "POLYMARKET-EXTRA"
type = "polymarket"
[data_clients.config]
subscribe_new_markets = false
"#
    );
    let errors = runtime_errors_for(&toml);
    assert_has_error(&errors, "data_clients", "duplicate_polymarket_client");
    let message = errors
        .iter()
        .find(|e| e.code == "duplicate_polymarket_client")
        .map(|e| e.message.clone())
        .unwrap_or_default();
    assert!(
        message.contains("polymarket ruleset validation:"),
        "error must carry the operator grep anchor: {message}"
    );
    assert!(
        message.contains("POLYMARKET") && message.contains("POLYMARKET-EXTRA"),
        "error should name both duplicate client names: {message}"
    );
}

#[test]
fn phase1_runtime_chainlink_requires_shared_reference_block() {
    let toml = replace(
        &strip_block(&valid_phase1_runtime_toml(), VALID_BINANCE_SHARED_BLOCK),
        r#"[[reference.venues]]
name = "BINANCE-BTC"
type = "binance"
instrument_id = "BTCUSDT.BINANCE"
base_weight = 0.35
stale_after_ms = 1500
disable_after_ms = 5000
"#,
        r#"[[reference.venues]]
name = "CHAINLINK-BTC"
type = "chainlink"
instrument_id = "BTCUSD.CHAINLINK"
base_weight = 0.35
stale_after_ms = 1500
disable_after_ms = 5000
[reference.venues.chainlink]
feed_id = "0x00036b4aa7e57ca7b68ae1bf45653f56b656fd3aa335ef7fae696b663f1b8472"
price_scale = 8
"#,
    )
    .replace(
        "resolution_basis = \"binance_btcusdt_1m\"",
        "resolution_basis = \"chainlink_btcusd\"",
    );
    let errors = runtime_errors_for(&toml);
    assert_has_error(&errors, "reference.chainlink", "missing_chainlink_config");
    assert_lacks_error(&errors, "reference.binance", "orphaned_binance_config");
}

#[test]
fn phase1_runtime_binance_requires_shared_reference_block() {
    let toml = strip_block(&valid_phase1_runtime_toml(), VALID_BINANCE_SHARED_BLOCK);
    let errors = runtime_errors_for(&toml);
    assert_has_error(&errors, "reference.binance", "missing_binance_config");
}

#[test]
fn phase1_runtime_binance_shared_paths_must_be_absolute_ssm_paths() {
    let toml = valid_phase1_runtime_toml()
        .replace("/bolt/binance/api-key", "bolt/binance/api-key")
        .replace("/bolt/binance/api-secret", "bolt/binance/api-secret");
    let errors = runtime_errors_for(&toml);
    assert_has_error(
        &errors,
        "reference.binance.api_key",
        "missing_leading_slash",
    );
    assert_has_error(
        &errors,
        "reference.binance.api_secret",
        "missing_leading_slash",
    );
}

#[test]
fn phase1_runtime_binance_allows_zero_instrument_status_poll_secs() {
    let toml = valid_phase1_runtime_toml().replace(
        "instrument_status_poll_secs = 3600",
        "instrument_status_poll_secs = 0",
    );
    let errors = runtime_errors_for(&toml);
    assert_lacks_error(
        &errors,
        "reference.binance.instrument_status_poll_secs",
        "not_positive",
    );
}

#[test]
fn phase1_runtime_binance_requires_non_empty_product_types() {
    let toml =
        valid_phase1_runtime_toml().replace("product_types = [\"SPOT\"]", "product_types = []");
    let errors = runtime_errors_for(&toml);
    assert_has_error(&errors, "reference.binance.product_types", "empty");
}

#[test]
fn phase1_runtime_binance_rejects_empty_base_url_http() {
    let toml = valid_phase1_runtime_toml().replace(
        "instrument_status_poll_secs = 3600\n",
        "instrument_status_poll_secs = 3600\nbase_url_http = \"\"\n",
    );
    let errors = runtime_errors_for(&toml);
    assert_has_error(&errors, "reference.binance.base_url_http", "empty");
}

#[test]
fn phase1_runtime_binance_rejects_invalid_base_url_http() {
    let toml = valid_phase1_runtime_toml().replace(
        "instrument_status_poll_secs = 3600\n",
        "instrument_status_poll_secs = 3600\nbase_url_http = \"not-a-url\"\n",
    );
    let errors = runtime_errors_for(&toml);
    assert_has_error(
        &errors,
        "reference.binance.base_url_http",
        "invalid_http_url",
    );
}

#[test]
fn phase1_runtime_binance_rejects_empty_base_url_ws() {
    let toml = valid_phase1_runtime_toml().replace(
        "instrument_status_poll_secs = 3600\n",
        "instrument_status_poll_secs = 3600\nbase_url_ws = \"\"\n",
    );
    let errors = runtime_errors_for(&toml);
    assert_has_error(&errors, "reference.binance.base_url_ws", "empty");
}

#[test]
fn phase1_runtime_binance_rejects_invalid_base_url_ws() {
    let toml = valid_phase1_runtime_toml().replace(
        "instrument_status_poll_secs = 3600\n",
        "instrument_status_poll_secs = 3600\nbase_url_ws = \"api.binance.com\"\n",
    );
    let errors = runtime_errors_for(&toml);
    assert_has_error(&errors, "reference.binance.base_url_ws", "invalid_ws_url");
}

#[test]
fn phase1_runtime_binance_shared_config_is_venue_driven_not_resolution_basis_driven() {
    let toml = format!(
        "{}\n{}",
        valid_phase1_runtime_toml()
            .replace("event_slugs = [\"btc-updown-5m\"]\n", "")
            .replace(
                "resolution_basis = \"binance_btcusdt_1m\"",
                "resolution_basis = \"chainlink_btcusd\"",
            )
            .replace(
                "[[reference.venues]]\nname = \"BINANCE-BTC\"",
                &format!(
                    "{VALID_CHAINLINK_SHARED_BLOCK}\n[[reference.venues]]\nname = \"BINANCE-BTC\""
                ),
            ),
        r#"
[[reference.venues]]
name = "CHAINLINK-BTC"
type = "chainlink"
instrument_id = "BTCUSD.CHAINLINK"
base_weight = 0.25
stale_after_ms = 1500
disable_after_ms = 5000
[reference.venues.chainlink]
feed_id = "0x00036b4aa7e57ca7b68ae1bf45653f56b656fd3aa335ef7fae696b663f1b8472"
price_scale = 8
"#
    );
    let errors = runtime_errors_for(&toml);
    assert!(
        errors.is_empty(),
        "a configured Binance reference venue remains valid even when the active ruleset resolves against another source: {errors:#?}"
    );
}

#[test]
fn phase1_runtime_chainlink_price_scale_must_be_positive_and_bounded() {
    let toml = replace(
        &strip_block(&valid_phase1_runtime_toml(), VALID_BINANCE_SHARED_BLOCK),
        r#"[[reference.venues]]
name = "BINANCE-BTC"
type = "binance"
instrument_id = "BTCUSDT.BINANCE"
base_weight = 0.35
stale_after_ms = 1500
disable_after_ms = 5000
"#,
        r#"[[reference.venues]]
name = "CHAINLINK-BTC"
type = "chainlink"
instrument_id = "BTCUSD.CHAINLINK"
base_weight = 0.35
stale_after_ms = 1000
disable_after_ms = 1500
[reference.venues.chainlink]
feed_id = "0x00036b4aa7e57ca7b68ae1bf45653f56b656fd3aa335ef7fae696b663f1b8472"
price_scale = 19
"#,
    )
    .replace(
        "[[reference.venues]]\nname = \"CHAINLINK-BTC\"",
        &format!("{VALID_CHAINLINK_SHARED_BLOCK}\n[[reference.venues]]\nname = \"CHAINLINK-BTC\""),
    )
    .replace(
        "resolution_basis = \"binance_btcusdt_1m\"",
        "resolution_basis = \"chainlink_btcusd\"",
    );
    let errors = runtime_errors_for(&toml);
    assert_has_error(
        &errors,
        "reference.venues[0].chainlink.price_scale",
        "too_large",
    );
}

#[test]
fn phase1_runtime_chainlink_price_scale_must_be_positive() {
    let toml = replace(
        &strip_block(&valid_phase1_runtime_toml(), VALID_BINANCE_SHARED_BLOCK),
        r#"[[reference.venues]]
name = "BINANCE-BTC"
type = "binance"
instrument_id = "BTCUSDT.BINANCE"
base_weight = 0.35
stale_after_ms = 1500
disable_after_ms = 5000
"#,
        r#"[[reference.venues]]
name = "CHAINLINK-BTC"
type = "chainlink"
instrument_id = "BTCUSD.CHAINLINK"
base_weight = 0.35
stale_after_ms = 1500
disable_after_ms = 5000
[reference.venues.chainlink]
feed_id = "0x00036b4aa7e57ca7b68ae1bf45653f56b656fd3aa335ef7fae696b663f1b8472"
price_scale = 0
"#,
    )
    .replace(
        "[[reference.venues]]\nname = \"CHAINLINK-BTC\"",
        &format!("{VALID_CHAINLINK_SHARED_BLOCK}\n[[reference.venues]]\nname = \"CHAINLINK-BTC\""),
    )
    .replace(
        "resolution_basis = \"binance_btcusdt_1m\"",
        "resolution_basis = \"chainlink_btcusd\"",
    );
    let errors = runtime_errors_for(&toml);
    assert_has_error(
        &errors,
        "reference.venues[0].chainlink.price_scale",
        "not_positive",
    );
}

#[test]
fn phase1_runtime_chainlink_feed_id_must_be_strict_hex() {
    let toml = replace(
        &strip_block(&valid_phase1_runtime_toml(), VALID_BINANCE_SHARED_BLOCK),
        r#"[[reference.venues]]
name = "BINANCE-BTC"
type = "binance"
instrument_id = "BTCUSDT.BINANCE"
base_weight = 0.35
stale_after_ms = 1500
disable_after_ms = 5000
"#,
        r#"[[reference.venues]]
name = "CHAINLINK-BTC"
type = "chainlink"
instrument_id = "BTCUSD.CHAINLINK"
base_weight = 0.35
stale_after_ms = 1500
disable_after_ms = 5000
[reference.venues.chainlink]
feed_id = "not-a-feed-id"
price_scale = 8
"#,
    )
    .replace(
        "[[reference.venues]]\nname = \"CHAINLINK-BTC\"",
        &format!("{VALID_CHAINLINK_SHARED_BLOCK}\n[[reference.venues]]\nname = \"CHAINLINK-BTC\""),
    )
    .replace(
        "resolution_basis = \"binance_btcusdt_1m\"",
        "resolution_basis = \"chainlink_btcusd\"",
    );
    let errors = runtime_errors_for(&toml);
    assert_has_error(
        &errors,
        "reference.venues[0].chainlink.feed_id",
        "invalid_feed_id",
    );
}

#[test]
fn phase1_runtime_chainlink_feed_ids_must_be_unique() {
    let toml = format!(
        "{}\n{}",
        strip_block(&valid_phase1_runtime_toml(), VALID_BINANCE_SHARED_BLOCK),
        r#"
[[reference.venues]]
name = "CHAINLINK-ETH"
type = "chainlink"
instrument_id = "ETHUSD.CHAINLINK"
base_weight = 0.25
stale_after_ms = 1500
disable_after_ms = 5000
[reference.venues.chainlink]
feed_id = "0x00036b4aa7e57ca7b68ae1bf45653f56b656fd3aa335ef7fae696b663f1b8472"
price_scale = 8
"#
    )
    .replace(
        r#"[[reference.venues]]
name = "BINANCE-BTC"
type = "binance"
instrument_id = "BTCUSDT.BINANCE"
base_weight = 0.35
stale_after_ms = 1500
disable_after_ms = 5000
"#,
        r#"[[reference.venues]]
name = "CHAINLINK-BTC"
type = "chainlink"
instrument_id = "BTCUSD.CHAINLINK"
base_weight = 0.35
stale_after_ms = 1500
disable_after_ms = 5000
[reference.venues.chainlink]
feed_id = "0x00036b4aa7e57ca7b68ae1bf45653f56b656fd3aa335ef7fae696b663f1b8472"
price_scale = 8
"#,
    )
    .replace(
        "resolution_basis = \"binance_btcusdt_1m\"",
        "resolution_basis = \"chainlink_btcusd\"",
    )
    .replace(
        "[[reference.venues]]\nname = \"CHAINLINK-BTC\"",
        &format!("{VALID_CHAINLINK_SHARED_BLOCK}\n[[reference.venues]]\nname = \"CHAINLINK-BTC\""),
    );
    let errors = runtime_errors_for(&toml);
    assert_has_error(&errors, "reference.venues", "duplicate_chainlink_feed_id");
}

#[test]
fn phase1_runtime_chainlink_shared_ws_reconnect_alert_threshold_must_be_positive() {
    let toml = replace(
        &strip_block(&valid_phase1_runtime_toml(), VALID_BINANCE_SHARED_BLOCK),
        r#"[[reference.venues]]
name = "BINANCE-BTC"
type = "binance"
instrument_id = "BTCUSDT.BINANCE"
base_weight = 0.35
stale_after_ms = 1500
disable_after_ms = 5000
"#,
        r#"[reference.chainlink]
region = "us-east-1"
api_key = "/bolt/chainlink/api_key"
api_secret = "/bolt/chainlink/api_secret"
ws_url = "wss://streams.chain.link"
ws_reconnect_alert_threshold = 0

[[reference.venues]]
name = "CHAINLINK-BTC"
type = "chainlink"
instrument_id = "BTCUSD.CHAINLINK"
base_weight = 0.35
stale_after_ms = 1500
disable_after_ms = 5000
[reference.venues.chainlink]
feed_id = "0x00036b4aa7e57ca7b68ae1bf45653f56b656fd3aa335ef7fae696b663f1b8472"
price_scale = 8
"#,
    )
    .replace(
        "resolution_basis = \"binance_btcusdt_1m\"",
        "resolution_basis = \"chainlink_btcusd\"",
    );
    let errors = runtime_errors_for(&toml);
    assert_has_error(
        &errors,
        "reference.chainlink.ws_reconnect_alert_threshold",
        "not_positive",
    );
}

#[test]
fn phase1_runtime_chainlink_shared_ws_url_must_be_wss() {
    let toml = replace(
        &strip_block(&valid_phase1_runtime_toml(), VALID_BINANCE_SHARED_BLOCK),
        r#"[[reference.venues]]
name = "BINANCE-BTC"
type = "binance"
instrument_id = "BTCUSDT.BINANCE"
base_weight = 0.35
stale_after_ms = 1500
disable_after_ms = 5000
"#,
        r#"[reference.chainlink]
region = "us-east-1"
api_key = "/bolt/chainlink/api_key"
api_secret = "/bolt/chainlink/api_secret"
ws_url = "ws://streams.chain.link"
ws_reconnect_alert_threshold = 5

[[reference.venues]]
name = "CHAINLINK-BTC"
type = "chainlink"
instrument_id = "BTCUSD.CHAINLINK"
base_weight = 0.35
stale_after_ms = 1500
disable_after_ms = 5000
[reference.venues.chainlink]
feed_id = "0x00036b4aa7e57ca7b68ae1bf45653f56b656fd3aa335ef7fae696b663f1b8472"
price_scale = 8
"#,
    )
    .replace(
        "resolution_basis = \"binance_btcusdt_1m\"",
        "resolution_basis = \"chainlink_btcusd\"",
    );
    let errors = runtime_errors_for(&toml);
    assert_has_error(&errors, "reference.chainlink.ws_url", "invalid_ws_url");
}

#[test]
fn phase1_runtime_chainlink_shared_ws_url_rejects_insecure_fallback_origin() {
    let toml = replace(
        &strip_block(&valid_phase1_runtime_toml(), VALID_BINANCE_SHARED_BLOCK),
        r#"[[reference.venues]]
name = "BINANCE-BTC"
type = "binance"
instrument_id = "BTCUSDT.BINANCE"
base_weight = 0.35
stale_after_ms = 1500
disable_after_ms = 5000
"#,
        r#"[reference.chainlink]
region = "us-east-1"
api_key = "/bolt/chainlink/api_key"
api_secret = "/bolt/chainlink/api_secret"
ws_url = "wss://primary.chain.link,ws://fallback.chain.link"
ws_reconnect_alert_threshold = 5

[[reference.venues]]
name = "CHAINLINK-BTC"
type = "chainlink"
instrument_id = "BTCUSD.CHAINLINK"
base_weight = 0.35
stale_after_ms = 1500
disable_after_ms = 5000
[reference.venues.chainlink]
feed_id = "0x00036b4aa7e57ca7b68ae1bf45653f56b656fd3aa335ef7fae696b663f1b8472"
price_scale = 8
"#,
    )
    .replace(
        "resolution_basis = \"binance_btcusdt_1m\"",
        "resolution_basis = \"chainlink_btcusd\"",
    );
    let errors = runtime_errors_for(&toml);
    assert_has_error(&errors, "reference.chainlink.ws_url", "invalid_ws_url");
}

#[test]
fn phase1_runtime_chainlink_shared_ws_url_must_include_host() {
    let toml = replace(
        &strip_block(&valid_phase1_runtime_toml(), VALID_BINANCE_SHARED_BLOCK),
        r#"[[reference.venues]]
name = "BINANCE-BTC"
type = "binance"
instrument_id = "BTCUSDT.BINANCE"
base_weight = 0.35
stale_after_ms = 1500
disable_after_ms = 5000
"#,
        r#"[reference.chainlink]
region = "us-east-1"
api_key = "/bolt/chainlink/api_key"
api_secret = "/bolt/chainlink/api_secret"
ws_url = "wss://"
ws_reconnect_alert_threshold = 5

[[reference.venues]]
name = "CHAINLINK-BTC"
type = "chainlink"
instrument_id = "BTCUSD.CHAINLINK"
base_weight = 0.35
stale_after_ms = 1500
disable_after_ms = 5000
[reference.venues.chainlink]
feed_id = "0x00036b4aa7e57ca7b68ae1bf45653f56b656fd3aa335ef7fae696b663f1b8472"
price_scale = 8
"#,
    )
    .replace(
        "resolution_basis = \"binance_btcusdt_1m\"",
        "resolution_basis = \"chainlink_btcusd\"",
    );
    let errors = runtime_errors_for(&toml);
    assert_has_error(&errors, "reference.chainlink.ws_url", "invalid_ws_url");
}
