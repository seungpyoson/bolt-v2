mod support;

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    thread,
    time::Duration,
};

use bolt_v2::{MaterializationOutcome, config::Config, materialize_live_config};
use support::{TempCaseDir, live_local_chainlink_operator_input, repo_path};

const CANONICAL_CHAINLINK_TESTNET_WS_URL: &str = "wss://ws.testnet-dataengine.chain.link";
const CORRECT_ETH_TESTNET_FEED_ID: &str =
    "0x000359843a543ee2fe414dc14c7e7920ef10f4372990b79d6361cdc0dd1ba782";

#[test]
fn materialize_live_config_creates_read_only_output() {
    let tempdir = TempCaseDir::new("create-output");
    let input_path = write_input(&tempdir, "live.local.toml");
    let output_path = tempdir.path().join("live.toml");

    let outcome = materialize_live_config(&input_path, &output_path)
        .expect("materializer should create the runtime config");

    assert_eq!(outcome, MaterializationOutcome::Created);
    assert!(output_path.exists());
    assert_generated_output(&output_path);
    assert_read_only(&output_path);
}

#[test]
fn materialize_live_config_creates_missing_parent_directories() {
    let tempdir = TempCaseDir::new("nested-output");
    let input_path = write_input(&tempdir, "live.local.toml");
    let output_path = tempdir.path().join("nested/runtime/live.toml");

    let outcome = materialize_live_config(&input_path, &output_path)
        .expect("materializer should create nested output directories");

    assert_eq!(outcome, MaterializationOutcome::Created);
    assert!(output_path.exists());
    assert_generated_output(&output_path);
    assert_read_only(&output_path);
}

#[test]
fn materialize_live_config_updates_drifted_contents() {
    let tempdir = TempCaseDir::new("updated-output");
    let input_path = write_input(&tempdir, "live.local.toml");
    let output_path = tempdir.path().join("live.toml");

    materialize_live_config(&input_path, &output_path).expect("first render should succeed");

    #[cfg(unix)]
    set_mode(&output_path, 0o600);
    #[cfg(not(unix))]
    make_writable_if_needed(&output_path);
    fs::write(&output_path, "drifted = true\n").expect("drifted output should be writable");

    let outcome = materialize_live_config(&input_path, &output_path)
        .expect("materializer should repair drifted contents");

    assert_eq!(outcome, MaterializationOutcome::Updated);
    assert_generated_output(&output_path);
    assert_read_only(&output_path);
    #[cfg(unix)]
    assert_mode(&output_path, 0o400);
}

#[test]
fn materialize_live_config_repairs_permissions_without_rewriting_contents() {
    let tempdir = TempCaseDir::new("permissions-repaired");
    let input_path = write_input(&tempdir, "live.local.toml");
    let output_path = tempdir.path().join("live.toml");

    materialize_live_config(&input_path, &output_path).expect("first render should succeed");

    let modified_before = fs::metadata(&output_path)
        .expect("output metadata should exist")
        .modified()
        .expect("output mtime should exist");
    thread::sleep(Duration::from_millis(20));

    #[cfg(unix)]
    set_mode(&output_path, 0o600);
    #[cfg(not(unix))]
    make_writable_if_needed(&output_path);

    let outcome = materialize_live_config(&input_path, &output_path)
        .expect("materializer should repair writable drift");

    let modified_after = fs::metadata(&output_path)
        .expect("output metadata should exist")
        .modified()
        .expect("output mtime should exist");

    assert_eq!(outcome, MaterializationOutcome::PermissionsRepaired);
    assert_eq!(
        modified_before, modified_after,
        "permission repair should not rewrite contents"
    );
    assert_generated_output(&output_path);
    assert_read_only(&output_path);
    #[cfg(unix)]
    assert_mode(&output_path, 0o400);
}

#[test]
fn materialize_live_config_leaves_matching_read_only_output_unchanged() {
    let tempdir = TempCaseDir::new("unchanged-output");
    let input_path = write_input(&tempdir, "live.local.toml");
    let output_path = tempdir.path().join("live.toml");

    materialize_live_config(&input_path, &output_path).expect("first render should succeed");

    let modified_before = fs::metadata(&output_path)
        .expect("output metadata should exist")
        .modified()
        .expect("output mtime should exist");
    thread::sleep(Duration::from_millis(20));

    let outcome =
        materialize_live_config(&input_path, &output_path).expect("second render should succeed");

    let modified_after = fs::metadata(&output_path)
        .expect("output metadata should exist")
        .modified()
        .expect("output mtime should exist");

    assert_eq!(outcome, MaterializationOutcome::Unchanged);
    assert_eq!(
        modified_before, modified_after,
        "unchanged output should not be rewritten"
    );
    assert_generated_output(&output_path);
    assert_read_only(&output_path);
}

#[test]
fn render_live_config_binary_supports_relative_paths() {
    let tempdir = TempCaseDir::new("relative-output");
    let input_path = write_input(&tempdir, "live.local.toml");
    let _ = input_path;

    let output = Command::new(env!("CARGO_BIN_EXE_render_live_config"))
        .current_dir(tempdir.path())
        .args(["--input", "live.local.toml", "--output", "live.toml"])
        .output()
        .expect("renderer binary should run");

    assert!(
        output.status.success(),
        "binary failed: stdout={}; stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let output_path = tempdir.path().join("live.toml");
    assert!(output_path.exists());
    assert_generated_output(&output_path);
    assert_read_only(&output_path);
}

#[test]
fn legacy_operator_config_without_phase1_sections_fails_closed() {
    let tempdir = TempCaseDir::new("legacy-phase1-compat");
    let input_path = tempdir.path().join("live.local.toml");
    let output_path = tempdir.path().join("live.toml");
    let input = r#"
[node]
name = "BOLT-V2-TEST"
trader_id = "BOLT-TEST"

[polymarket]
event_slug = "btc-updown-5m"
instrument_id = "0xabc-12345678901234567890.POLYMARKET"
account_id = "POLYMARKET-001"
funder = "0xabc"

[secrets]
pk = "/bolt/poly/pk"
api_key = "/bolt/poly/key"
api_secret = "/bolt/poly/secret"
passphrase = "/bolt/poly/passphrase"
# This legacy shape still needs an explicit raw_capture path so the fail-closed
# check stays focused on the missing active runtime path.
[raw_capture]
output_dir = "/srv/bolt-v2/var/raw"
"#;

    fs::write(&input_path, input).expect("input config should be written");

    let error = materialize_live_config(&input_path, &output_path)
        .expect_err("legacy operator config should fail closed without an active runtime path")
        .to_string();

    assert!(
        error.contains("at least one ruleset or strategy"),
        "expected fail-closed runtime-shape error, got: {error}"
    );
    assert!(!output_path.exists());
}

#[test]
fn materialize_live_config_rejects_rendered_runtime_validation_failures() {
    let tempdir = TempCaseDir::new("runtime-validation-failure");
    let input_path = tempdir.path().join("live.local.toml");
    let output_path = tempdir.path().join("live.toml");
    let input = r#"
[node]
name = "BOLT-V2-TEST"
trader_id = "BOLT-TEST"

[polymarket]
instrument_id = "0xabc-12345678901234567890.POLYMARKET"
account_id = "POLYMARKET-001"
funder = "0xabc"

[secrets]
pk = "/bolt/poly/pk"
api_key = "/bolt/poly/key"
api_secret = "/bolt/poly/secret"
passphrase = "/bolt/poly/passphrase"

[raw_capture]
output_dir = "/srv/bolt-v2/var/raw"

[reference]
publish_topic = "platform.reference.default"
min_publish_interval_ms = 100

[reference.binance]
region = "eu-west-1"
api_key = "/bolt/binance/api-key"
api_secret = "/bolt/binance/api-secret"
environment = "Mainnet"
product_types = ["SPOT"]
instrument_status_poll_secs = 3600

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
resolution_basis = "kraken_btcusd_1m"
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
"#;

    fs::write(&input_path, input).expect("input config should be written");

    let error = materialize_live_config(&input_path, &output_path)
        .expect_err("runtime-invalid rendered config should fail materialization")
        .to_string();

    assert!(
        error.contains("Runtime config validation failed"),
        "unexpected materialization error: {error}"
    );
    assert!(
        error.contains("rulesets[0].resolution_basis"),
        "runtime validation error should mention resolution_basis: {error}"
    );
    assert!(
        !output_path.exists(),
        "materialization should not write output on runtime validation failure"
    );
}

#[test]
fn materialize_live_config_renders_position_check_interval_when_configured() {
    let tempdir = TempCaseDir::new("position-check-render");
    let input_path = tempdir.path().join("live.local.toml");
    let output_path = tempdir.path().join("live.toml");
    let input = r#"
[node]
name = "BOLT-V2-TEST"
trader_id = "BOLT-TEST"

[exec_engine]
position_check_interval_secs = 19

[polymarket]
instrument_id = "0xabc-12345678901234567890.POLYMARKET"
account_id = "POLYMARKET-001"
funder = "0xabc"

[secrets]
pk = "/bolt/poly/pk"
api_key = "/bolt/poly/key"
api_secret = "/bolt/poly/secret"
passphrase = "/bolt/poly/passphrase"

[raw_capture]
output_dir = "/srv/bolt-v2/var/raw"

[reference]
publish_topic = "platform.reference.default"
min_publish_interval_ms = 100

[reference.binance]
region = "eu-west-1"
api_key = "/bolt/binance/api-key"
api_secret = "/bolt/binance/api-secret"
environment = "Mainnet"
product_types = ["SPOT"]
instrument_status_poll_secs = 3600

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
"#;

    fs::write(&input_path, input).expect("input config should be written");

    materialize_live_config(&input_path, &output_path)
        .expect("materializer should render position check interval");

    let rendered =
        fs::read_to_string(&output_path).expect("materialized config should be readable");
    let cfg = Config::load(&output_path).expect("materialized config should parse");

    assert!(rendered.contains("position_check_interval_secs = 19"));
    assert_eq!(cfg.exec_engine.position_check_interval_secs, Some(19.0));
}

#[test]
fn materialize_live_config_rejects_legacy_strategy_block() {
    let tempdir = TempCaseDir::new("legacy-strategy-input-rejected");
    let input_path = tempdir.path().join("live.local.toml");
    let output_path = tempdir.path().join("live.toml");
    let input = r#"
[node]
name = "BOLT-V2-TEST"
trader_id = "BOLT-TEST"

[polymarket]
instrument_id = "0xabc-12345678901234567890.POLYMARKET"
account_id = "POLYMARKET-001"
funder = "0xabc"

[strategy]
order_qty = "10"

[secrets]
pk = "/bolt/poly/pk"
api_key = "/bolt/poly/key"
api_secret = "/bolt/poly/secret"
passphrase = "/bolt/poly/passphrase"

[raw_capture]
output_dir = "/srv/bolt-v2/var/raw"

[reference]
publish_topic = "platform.reference.default"
min_publish_interval_ms = 100

[reference.binance]
region = "eu-west-1"
api_key = "/bolt/binance/api-key"
api_secret = "/bolt/binance/api-secret"
environment = "Mainnet"
product_types = ["SPOT"]
instrument_status_poll_secs = 3600

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
"#;

    fs::write(&input_path, input).expect("input config should be written");

    let error = materialize_live_config(&input_path, &output_path)
        .expect_err("legacy live-local strategy block should be rejected")
        .to_string();

    assert!(
        error.contains("unknown field `strategy`"),
        "expected parse error for legacy strategy block, got: {error}"
    );
    assert!(!output_path.exists());
}

#[test]
fn render_live_config_binary_resolves_contract_path_from_repo_root() {
    let tempdir = TempCaseDir::new("relative-contract-root");
    std::fs::write(
        tempdir.path().join("Cargo.toml"),
        "[package]\nname = \"temp\"\n",
    )
    .expect("repo marker should be written");
    std::fs::create_dir_all(tempdir.path().join("config")).expect("config dir should exist");
    std::fs::create_dir_all(tempdir.path().join("contracts")).expect("contracts dir should exist");
    std::fs::write(
        tempdir.path().join("contracts/polymarket.toml"),
        "schema_version = 1\nvenue = \"test\"\nadapter_version = \"bolt-v2\"\n\n\
         [streams.quotes]\ncapability = \"supported\"\npolicy = \"required\"\n\n\
         [streams.trades]\ncapability = \"supported\"\npolicy = \"required\"\n\n\
         [streams.order_book_deltas]\ncapability = \"supported\"\npolicy = \"required\"\n\n\
         [streams.order_book_depths]\ncapability = \"unsupported\"\n\n\
         [streams.index_prices]\ncapability = \"unsupported\"\n\n\
         [streams.mark_prices]\ncapability = \"unsupported\"\n\n\
         [streams.instrument_closes]\ncapability = \"unsupported\"\n",
    )
    .expect("contract fixture should be written");

    let source = tracked_live_local_example()
        .replace(
            "# contract_path = \"contracts/polymarket.toml\"",
            "contract_path = \"contracts/polymarket.toml\"",
        )
        .replace("catalog_path = \"\"", "catalog_path = \"var/catalog\"");
    fs::write(tempdir.path().join("config/live.local.toml"), source)
        .expect("input config should be written");

    let output = Command::new(env!("CARGO_BIN_EXE_render_live_config"))
        .current_dir(tempdir.path())
        .args(["--input", "config/live.local.toml", "--output", "live.toml"])
        .output()
        .expect("renderer binary should run");

    assert!(
        output.status.success(),
        "binary failed: stdout={}; stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let rendered = fs::read_to_string(tempdir.path().join("live.toml"))
        .expect("rendered config should be readable");
    let cfg: Config = toml::from_str(&rendered).expect("rendered config should parse");
    let expected_root = fs::canonicalize(tempdir.path()).expect("tempdir should resolve");
    assert_eq!(
        cfg.streaming.contract_path.as_deref(),
        Some(
            expected_root
                .join("contracts/polymarket.toml")
                .to_str()
                .expect("absolute contract path should be utf-8")
        )
    );
}

#[cfg(unix)]
#[test]
fn render_live_config_binary_respects_restrictive_umask_on_create() {
    let tempdir = TempCaseDir::new("umask-create");
    let _input_path = write_input(&tempdir, "live.local.toml");

    let output = Command::new("/bin/sh")
        .current_dir(tempdir.path())
        .env("BIN", env!("CARGO_BIN_EXE_render_live_config"))
        .env("INPUT", "live.local.toml")
        .env("OUTPUT", "live.toml")
        .arg("-c")
        .arg("umask 077 && \"$BIN\" --input \"$INPUT\" --output \"$OUTPUT\"")
        .output()
        .expect("renderer binary should run with restrictive umask");

    assert!(
        output.status.success(),
        "binary failed: stdout={}; stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let output_path = tempdir.path().join("live.toml");
    assert_generated_output(&output_path);
    assert_read_only(&output_path);
    assert_mode(&output_path, 0o400);
}

#[test]
fn materialize_live_config_renders_nested_chainlink_reference_settings() {
    let tempdir = TempCaseDir::new("render-chainlink-reference");
    let input_path = tempdir.path().join("live.local.toml");
    let output_path = tempdir.path().join("live.toml");
    let input = live_local_chainlink_operator_input();

    fs::write(&input_path, input).expect("input config should be written");
    materialize_live_config(&input_path, &output_path)
        .expect("chainlink operator config should materialize");

    let rendered = fs::read_to_string(&output_path).expect("rendered config should be readable");
    let cfg: Config = toml::from_str(&rendered).expect("rendered config should parse");
    let shared = cfg
        .reference
        .chainlink
        .as_ref()
        .expect("rendered shared chainlink config should be present");
    let chainlink = cfg.reference.venues[0]
        .chainlink
        .as_ref()
        .expect("rendered chainlink config should be present");

    assert!(rendered.contains("[reference.chainlink]"));
    assert!(rendered.contains("[reference.venues.chainlink]"));
    assert_eq!(shared.region, "us-east-1");
    assert_eq!(shared.api_key, "/bolt/chainlink/api_key");
    assert_eq!(shared.api_secret, "/bolt/chainlink/api_secret");
    assert_eq!(shared.ws_url, CANONICAL_CHAINLINK_TESTNET_WS_URL);
    assert_eq!(shared.ws_reconnect_alert_threshold, 5);
    assert_eq!(
        chainlink.feed_id,
        "0x00036b4aa7e57ca7b68ae1bf45653f56b656fd3aa335ef7fae696b663f1b8472"
    );
    assert_eq!(chainlink.price_scale, 8);
}

#[test]
fn materialize_live_config_preserves_valid_chainlink_ws_fallback_origins() {
    let tempdir = TempCaseDir::new("render-chainlink-ws-fallback-origins");
    let input_path = tempdir.path().join("live.local.toml");
    let output_path = tempdir.path().join("live.toml");
    let input = live_local_chainlink_operator_input().replace(
        "ws_url = \"wss://ws.testnet-dataengine.chain.link\"",
        "ws_url = \"wss://primary.chain.link,wss://fallback.chain.link\"",
    );

    fs::write(&input_path, input).expect("input config should be written");
    materialize_live_config(&input_path, &output_path)
        .expect("chainlink operator config with ws fallback origins should materialize");

    let rendered = fs::read_to_string(&output_path).expect("rendered config should be readable");
    let cfg: Config = toml::from_str(&rendered).expect("rendered config should parse");
    let shared = cfg
        .reference
        .chainlink
        .as_ref()
        .expect("rendered shared chainlink config should be present");

    assert_eq!(
        shared.ws_url,
        "wss://primary.chain.link,wss://fallback.chain.link"
    );
}

#[test]
fn materialize_live_config_rejects_invalid_chainlink_ws_fallback_origin() {
    let tempdir = TempCaseDir::new("invalid-chainlink-ws-fallback-origin");
    let input_path = tempdir.path().join("live.local.toml");
    let output_path = tempdir.path().join("live.toml");
    let input = live_local_chainlink_operator_input().replace(
        "ws_url = \"wss://ws.testnet-dataengine.chain.link\"",
        "ws_url = \"wss://primary.chain.link,ws://fallback.chain.link\"",
    );

    fs::write(&input_path, input).expect("input config should be written");
    let error = materialize_live_config(&input_path, &output_path)
        .expect_err("invalid chainlink ws fallback origin should fail validation")
        .to_string();

    assert!(error.contains("reference.chainlink.ws_url"));
    assert!(error.contains("must start with wss://"));
}

fn write_input(tempdir: &TempCaseDir, file_name: &str) -> PathBuf {
    let input_path = tempdir.path().join(file_name);
    fs::write(&input_path, tracked_live_local_example()).expect("input config should be written");
    input_path
}

fn tracked_live_local_example() -> String {
    fs::read_to_string(repo_path("config/live.local.example.toml"))
        .expect("tracked operator template should be readable")
}

#[test]
fn eth_ruleset_operator_input_materializes_taker_runtime_template() {
    let tempdir = TempCaseDir::new("eth-ruleset-runtime-template");
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

[raw_capture]
output_dir = "/srv/bolt-v2/var/raw"

[reference]
publish_topic = "platform.reference.default"
min_publish_interval_ms = 100

[reference.chainlink]
region = "eu-west-1"
api_key = "/bolt/chainlink/api-key"
api_secret = "/bolt/chainlink/api-secret"
ws_url = "wss://ws.testnet-dataengine.chain.link"
ws_reconnect_alert_threshold = 5

[[reference.venues]]
name = "CHAINLINK-ETH"
type = "chainlink"
instrument_id = "ETHUSD.CHAINLINK"
base_weight = 1.0
stale_after_ms = 1500
disable_after_ms = 5000
[reference.venues.chainlink]
feed_id = "0x000359843a543ee2fe414dc14c7e7920ef10f4372990b79d6361cdc0dd1ba782"
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
local_dir = "/srv/bolt-v2/var/audit"
s3_uri = "s3://bolt-runtime-history/phase1"
ship_interval_secs = 30
upload_attempt_timeout_secs = 30
roll_max_bytes = 1048576
roll_max_secs = 300
max_local_backlog_bytes = 10485760
"#;

    fs::write(&input_path, input).expect("input config should be written");

    materialize_live_config(&input_path, &output_path)
        .expect("ruleset operator input should materialize");

    let rendered = fs::read_to_string(&output_path).expect("rendered config should be readable");
    let cfg: Config = toml::from_str(&rendered).expect("rendered config should parse");
    let shared = cfg
        .reference
        .chainlink
        .as_ref()
        .expect("rendered shared chainlink config should be present");
    let chainlink = cfg.reference.venues[0]
        .chainlink
        .as_ref()
        .expect("rendered chainlink config should be present");

    assert!(
        rendered.contains("[[strategies]]"),
        "eth taker operator config should render a runtime strategy template"
    );
    assert_eq!(shared.ws_url, CANONICAL_CHAINLINK_TESTNET_WS_URL);
    assert_eq!(chainlink.feed_id, CORRECT_ETH_TESTNET_FEED_ID);
    assert_eq!(cfg.strategies.len(), 1);
    assert_eq!(cfg.strategies[0].kind, "eth_chainlink_taker");
    assert_eq!(
        cfg.strategies[0].config["strategy_id"].as_str(),
        Some("ETHCHAINLINKTAKER-001")
    );
}

fn assert_generated_output(path: &Path) {
    let rendered = fs::read_to_string(path).expect("generated output should be readable");
    let cfg: Config = toml::from_str(&rendered).expect("generated output should parse");

    assert!(rendered.contains("# GENERATED FILE - DO NOT EDIT."));
    assert!(rendered.contains("# Source of truth:"));
    assert!(rendered.contains("[[data_clients]]"));
    assert!(rendered.contains("[[exec_clients]]"));
    assert!(cfg.strategies.is_empty());
    assert!(!rendered.contains("[[strategies]]"));
}

fn assert_read_only(path: &Path) {
    let metadata = fs::metadata(path).expect("output metadata should exist");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        assert_eq!(metadata.permissions().mode() & 0o222, 0);
    }

    #[cfg(not(unix))]
    {
        assert!(metadata.permissions().readonly());
    }
}

#[cfg(not(unix))]
fn make_writable_if_needed(path: &Path) {
    let mut permissions = fs::metadata(path)
        .expect("output metadata should exist")
        .permissions();
    permissions.set_readonly(false);
    fs::set_permissions(path, permissions).expect("output should become writable");
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;

    let mut permissions = fs::metadata(path)
        .expect("output metadata should exist")
        .permissions();
    permissions.set_mode(mode);
    fs::set_permissions(path, permissions).expect("output mode should be updated");
}

#[cfg(unix)]
fn assert_mode(path: &Path, expected_mode: u32) {
    use std::os::unix::fs::PermissionsExt;

    let actual_mode = fs::metadata(path)
        .expect("output metadata should exist")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(actual_mode, expected_mode);
}
