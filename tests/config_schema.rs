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
"#;

    let cfg: Config = toml::from_str(raw).expect("config should parse");
    assert_eq!(cfg.data_clients.len(), 1);
    assert_eq!(cfg.exec_clients.len(), 1);
    assert_eq!(cfg.strategies.len(), 0);
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
    assert_eq!(cfg.strategies.len(), 0);
    assert_eq!(cfg.exec_engine.position_check_interval_secs, None);
    assert_eq!(cfg.data_clients[0].kind, "polymarket");
    assert_eq!(cfg.exec_clients[0].kind, "polymarket");
    assert!(
        !rendered.contains("[[strategies]]"),
        "tracked example has no active strategy templates (all commented out), so rendered config should contain no [[strategies]] section"
    );
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
fn tracked_ruleset_template_materializes_runtime_strategy_template() {
    let tempdir = TempCaseDir::new("config-schema-ruleset-template");
    let input_path = tempdir.path().join("live.local.toml");
    let output_path = tempdir.path().join("live.toml");
    let input = r#"
[node]
name = "BOLT-V2-ETH-001"
trader_id = "BOLT-ETH-001"

[polymarket]
instrument_id = "0x8213d395e079614d6c4d7f4cbb9be9337ab51648a21cc2a334ae8f1966d164b4-111128191581505463501777127559667396812474366956707382672202929745167742497287.POLYMARKET"
account_id = "POLYMARKET-001"
funder = "0xA3a5E9c062331237E5f1403b2bba7A184e5de983"
signature_type = 2

[secrets]
region = "eu-west-1"
pk = "/bolt/polymarket/private-key"
api_key = "/bolt/polymarket/api-key"
api_secret = "/bolt/polymarket/api-secret"
passphrase = "/bolt/polymarket/api-passphrase"

[reference]
publish_topic = "platform.reference.default"
min_publish_interval_ms = 100

[reference.chainlink]
region = "eu-west-1"
api_key = "/bolt/chainlink/api-key"
api_secret = "/bolt/chainlink/api-secret"
ws_url = "wss://streams.chain.link"
ws_reconnect_alert_threshold = 5

[[reference.venues]]
name = "CHAINLINK-ETH"
type = "chainlink"
instrument_id = "ETHUSD.CHAINLINK"
base_weight = 1.0
stale_after_ms = 1500
disable_after_ms = 5000
[reference.venues.chainlink]
feed_id = "0x00037da06d56d083fe599397a4769a042d63aa73dc4ef57709d31e9971a5b439"
price_scale = 18

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

[[rulesets]]
id = "ETHCHAINLINKTAKER"
venue = "polymarket"
resolution_basis = "chainlink_ethusd"
min_time_to_expiry_secs = 60
max_time_to_expiry_secs = 900
min_liquidity_num = 1000
require_accepting_orders = true
freeze_before_end_secs = 90
selector_poll_interval_ms = 1000
candidate_load_timeout_secs = 30

[rulesets.selector]
tag_slug = "ethereum"

[audit]
local_dir = "var/audit"
s3_uri = "s3://bolt-runtime-history/phase1"
ship_interval_secs = 30
upload_attempt_timeout_secs = 30
roll_max_bytes = 1048576
roll_max_secs = 300
max_local_backlog_bytes = 10485760
"#;

    fs::write(&input_path, input).expect("input config should be written");

    materialize_live_config(&input_path, &output_path)
        .expect("ruleset operator config should materialize");

    let cfg = Config::load(&output_path).expect("materialized config should parse");

    assert_eq!(
        cfg.strategies.len(),
        1,
        "ruleset-backed taker operator config should materialize one runtime strategy template"
    );
    assert_eq!(cfg.strategies[0].kind, "eth_chainlink_taker");
    assert_eq!(
        cfg.strategies[0].config["strategy_id"].as_str(),
        Some("ETHCHAINLINKTAKER-001")
    );
    assert_eq!(
        cfg.strategies[0].config["client_id"].as_str(),
        Some("POLYMARKET")
    );
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
    assert!(
        value.get("strategies").is_none(),
        "ruleset mode should omit runtime strategy templates from rendered TOML"
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

#[test]
fn rendered_operator_config_passes_through_position_check_interval() {
    let tempdir = TempCaseDir::new("position-check-config-schema");
    let input_path = tempdir.path().join("live.local.toml");
    let output_path = tempdir.path().join("live.toml");
    let input = r#"
[node]
name = "BOLT-V2-001"
trader_id = "BOLT-001"

[exec_engine]
position_check_interval_secs = 23

[polymarket]
instrument_id = "0xabc-123.POLYMARKET"
account_id = "POLYMARKET-001"
funder = "0xabc"
signature_type = 2

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
        .expect("operator config should materialize");

    assert_eq!(outcome, MaterializationOutcome::Created);

    let cfg = Config::load(&output_path).expect("materialized config should parse");

    assert_eq!(cfg.exec_engine.position_check_interval_secs, Some(23.0));
}
