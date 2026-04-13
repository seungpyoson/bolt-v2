mod support;

use bolt_v2::config::Config;
use bolt_v2::{MaterializationOutcome, materialize_live_config};
use std::fs;
use support::{TempCaseDir, repo_path};
use toml::Value;

#[test]
fn parses_minimal_polymarket_wrapper_config() {
    let raw = r#"
[node]
name = "BOLT-V2-001"
trader_id = "BOLT-001"
environment = "Live"
load_state = false
save_state = false
timeout_connection_secs = 60
timeout_reconciliation_secs = 30
timeout_portfolio_secs = 10
timeout_disconnection_secs = 10
delay_post_stop_secs = 10
delay_shutdown_secs = 5

[logging]
stdout_level = "Info"
file_level = "Debug"

[[data_clients]]
name = "POLYMARKET"
type = "polymarket"
[data_clients.config]
subscribe_new_markets = false
update_instruments_interval_mins = 60
ws_max_subscriptions = 200
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
instrument_id = "TOKEN.POLYMARKET"
client_id = "POLYMARKET"
order_qty = "5"
log_data = false
tob_offset_ticks = 5
use_post_only = true
enable_limit_sells = false
enable_stop_buys = false
enable_stop_sells = false
"#;

    let cfg: Config = toml::from_str(raw).expect("config should parse");
    assert_eq!(cfg.data_clients.len(), 1);
    assert_eq!(cfg.exec_clients.len(), 1);
    assert_eq!(cfg.strategies.len(), 1);
}

#[test]
fn tracked_template_materializes_to_parseable_runtime_config() {
    let tempdir = TempCaseDir::new("config-schema");
    let output_path = tempdir.path().join("live.toml");
    let source_path = repo_path("config/live.local.example.toml");

    let outcome = materialize_live_config(&source_path, &output_path)
        .expect("tracked template should materialize");

    assert_eq!(outcome, MaterializationOutcome::Created);

    let rendered =
        fs::read_to_string(&output_path).expect("materialized config should be readable");
    let cfg = Config::load(&output_path).expect("materialized config should parse");

    assert!(rendered.contains(&format!("# Source of truth: {}", source_path.display())));
    assert!(!rendered.contains("Regenerate with:"));
    assert_eq!(cfg.data_clients.len(), 1);
    assert_eq!(cfg.exec_clients.len(), 1);
    assert_eq!(cfg.strategies.len(), 1);
    assert_eq!(cfg.data_clients[0].kind, "polymarket");
    assert_eq!(cfg.exec_clients[0].kind, "polymarket");
    assert_eq!(cfg.strategies[0].kind, "exec_tester");
    assert!(
        cfg.data_clients[0].config.get("event_slugs").is_none(),
        "ruleset mode should not materialize legacy event_slugs into runtime data-client config"
    );
    assert_eq!(
        cfg.data_clients[0].config["gamma_refresh_interval_secs"].as_integer(),
        Some(60)
    );
    assert_eq!(cfg.rulesets[0].selector_poll_interval_ms, 1_000);
    assert_eq!(cfg.rulesets[0].candidate_load_timeout_secs, 30);
    assert_eq!(cfg.audit.as_ref().unwrap().upload_attempt_timeout_secs, 30);
}

#[test]
fn rendered_operator_config_supports_reference_rulesets_array_and_optional_audit() {
    let tempdir = TempCaseDir::new("phase1-config-schema");
    let input_path = tempdir.path().join("live.local.toml");
    let output_path = tempdir.path().join("live.toml");
    let input = r#"
[node]
name = "BOLT-V2-001"
trader_id = "BOLT-001"

[polymarket]
instrument_id = "0xabc-123.POLYMARKET"
account_id = "POLYMARKET-001"
funder = "0xabc"
signature_type = 2

[strategy]
strategy_id = "EXEC_TESTER-001"
order_qty = "5"

[secrets]
region = "eu-west-1"
pk = "/bolt/poly/pk"
api_key = "/bolt/poly/key"
api_secret = "/bolt/poly/secret"
passphrase = "/bolt/poly/passphrase"

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
selector_poll_interval_ms = 250
candidate_load_timeout_secs = 12

[rulesets.selector]
tag_slug = "bitcoin"

[audit]
local_dir = "var/audit"
s3_uri = "s3://bolt-runtime-history/phase1"
ship_interval_secs = 30
upload_attempt_timeout_secs = 45
roll_max_bytes = 1048576
roll_max_secs = 300
max_local_backlog_bytes = 10485760
"#;

    fs::write(&input_path, input).expect("input config should be written");

    let outcome = materialize_live_config(&input_path, &output_path)
        .expect("phase1 operator config should materialize");

    assert_eq!(outcome, MaterializationOutcome::Created);

    let rendered =
        fs::read_to_string(&output_path).expect("materialized config should be readable");
    let value: Value = toml::from_str(&rendered).expect("rendered config should stay valid TOML");

    assert_eq!(
        value["reference"]["publish_topic"].as_str(),
        Some("platform.reference.default")
    );
    assert_eq!(
        value["reference"]["min_publish_interval_ms"].as_integer(),
        Some(100)
    );
    assert_eq!(
        value["reference"]["venues"].as_array().map(Vec::len),
        Some(1)
    );
    assert_eq!(value["rulesets"].as_array().map(Vec::len), Some(1));
    assert_eq!(value["rulesets"][0]["venue"].as_str(), Some("polymarket"));
    assert_eq!(
        value["rulesets"][0]["selector_poll_interval_ms"].as_integer(),
        Some(250)
    );
    assert_eq!(
        value["rulesets"][0]["candidate_load_timeout_secs"].as_integer(),
        Some(12)
    );
    assert_eq!(value["audit"]["local_dir"].as_str(), Some("var/audit"));
    assert_eq!(
        value["audit"]["upload_attempt_timeout_secs"].as_integer(),
        Some(45)
    );
    assert_eq!(
        value["audit"]["max_local_backlog_bytes"].as_integer(),
        Some(10_485_760)
    );
}
