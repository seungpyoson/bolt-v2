mod support;

use bolt_v2::config::Config;
use bolt_v2::{MaterializationOutcome, materialize_live_config};
use std::fs;
use support::{TempCaseDir, repo_path};
use toml::Value;

#[test]
fn parses_minimal_polymarket_wrapper_config() {
    let raw = r#"
strategies = []

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
"#;

    let cfg: Config = toml::from_str(raw).expect("config should parse");
    assert_eq!(cfg.data_clients.len(), 1);
    assert_eq!(cfg.exec_clients.len(), 1);
    assert_eq!(cfg.strategies.len(), 0);
}
