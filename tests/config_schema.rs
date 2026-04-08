mod support;

use bolt_v2::config::Config;
use bolt_v2::{MaterializationOutcome, materialize_live_config};
use std::{
    fs,
    path::Path,
};
use support::TempCaseDir;

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

    let outcome = materialize_live_config(Path::new("config/live.local.example.toml"), &output_path)
        .expect("tracked template should materialize");

    assert_eq!(outcome, MaterializationOutcome::Created);

    let rendered = fs::read_to_string(&output_path).expect("materialized config should be readable");
    let cfg = Config::load(&output_path).expect("materialized config should parse");

    assert!(rendered.contains("# Source of truth: config/live.local.example.toml"));
    assert!(!rendered.contains("Regenerate with:"));
    assert_eq!(cfg.data_clients.len(), 1);
    assert_eq!(cfg.exec_clients.len(), 1);
    assert_eq!(cfg.strategies.len(), 1);
    assert_eq!(cfg.data_clients[0].kind, "polymarket");
    assert_eq!(cfg.exec_clients[0].kind, "polymarket");
    assert_eq!(cfg.strategies[0].kind, "exec_tester");
}
