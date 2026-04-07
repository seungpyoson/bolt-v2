use std::{
    fs,
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

static TEMP_CONFIG_COUNTER: AtomicU64 = AtomicU64::new(0);

#[test]
fn temp_config_paths_remain_unique_for_same_timestamp() {
    let first = temp_config_path_for_timestamp(123);
    let second = temp_config_path_for_timestamp(123);

    assert_ne!(first, second);
}

#[test]
fn secrets_check_reports_complete_secret_config() {
    let output = Command::new(env!("CARGO_BIN_EXE_bolt-v2"))
        .args([
            "secrets",
            "check",
            "--config",
            "config/examples/polymarket-exec-tester.toml",
        ])
        .output()
        .expect("secrets check should run");

    assert!(output.status.success());

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains(
        "POLYMARKET: secret config complete (region, pk, api_key, api_secret, passphrase)"
    ));
}

#[test]
fn secrets_check_fails_when_required_fields_are_missing() {
    let path = write_temp_config(
        r#"
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
name = "TEST"
type = "polymarket"
[data_clients.config]

[[exec_clients]]
name = "TEST"
type = "polymarket"
[exec_clients.config]
account_id = "TEST-001"
signature_type = 2
funder = "0xabc"
[exec_clients.secrets]
region = "eu-west-1"

[[strategies]]
type = "exec_tester"
[strategies.config]
strategy_id = "EXEC_TESTER-001"
instrument_id = "TOKEN.TEST"
client_id = "TEST"
order_qty = "5"
"#,
    );

    let output = Command::new(env!("CARGO_BIN_EXE_bolt-v2"))
        .args([
            "secrets",
            "check",
            "--config",
            path.to_str().expect("utf-8 path"),
        ])
        .output()
        .expect("secrets check should run");

    assert!(!output.status.success());

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("TEST: missing secret config fields (pk, api_key, api_secret, passphrase)")
    );
}

#[test]
fn secrets_resolve_fails_fast_when_required_fields_are_missing() {
    let path = write_temp_config(
        r#"
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
name = "TEST"
type = "polymarket"
[data_clients.config]

[[exec_clients]]
name = "TEST"
type = "polymarket"
[exec_clients.config]
account_id = "TEST-001"
signature_type = 2
funder = "0xabc"
[exec_clients.secrets]
region = "eu-west-1"

[[strategies]]
type = "exec_tester"
[strategies.config]
strategy_id = "EXEC_TESTER-001"
instrument_id = "TOKEN.TEST"
client_id = "TEST"
order_qty = "5"
"#,
    );

    let output = Command::new(env!("CARGO_BIN_EXE_bolt-v2"))
        .args([
            "secrets",
            "resolve",
            "--config",
            path.to_str().expect("utf-8 path"),
        ])
        .output()
        .expect("secrets resolve should run");

    assert!(!output.status.success());

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr
            .contains("Missing required secret config fields: pk, api_key, api_secret, passphrase")
    );
}

#[test]
fn secrets_check_fails_when_region_is_blank() {
    let path = write_temp_config(
        r#"
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
name = "TEST"
type = "polymarket"
[data_clients.config]

[[exec_clients]]
name = "TEST"
type = "polymarket"
[exec_clients.config]
account_id = "TEST-001"
signature_type = 2
funder = "0xabc"
[exec_clients.secrets]
region = ""
pk = "/x/pk"
api_key = "/x/key"
api_secret = "/x/secret"
passphrase = "/x/pass"

[[strategies]]
type = "exec_tester"
[strategies.config]
strategy_id = "EXEC_TESTER-001"
instrument_id = "TOKEN.TEST"
client_id = "TEST"
order_qty = "5"
"#,
    );

    let output = Command::new(env!("CARGO_BIN_EXE_bolt-v2"))
        .args([
            "secrets",
            "check",
            "--config",
            path.to_str().expect("utf-8 path"),
        ])
        .output()
        .expect("secrets check should run");

    assert!(!output.status.success());

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("TEST: missing secret config fields (region)"));
}

#[test]
fn secrets_resolve_fails_fast_when_region_is_blank() {
    let path = write_temp_config(
        r#"
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
name = "TEST"
type = "polymarket"
[data_clients.config]

[[exec_clients]]
name = "TEST"
type = "polymarket"
[exec_clients.config]
account_id = "TEST-001"
signature_type = 2
funder = "0xabc"
[exec_clients.secrets]
region = ""
pk = "/x/pk"
api_key = "/x/key"
api_secret = "/x/secret"
passphrase = "/x/pass"

[[strategies]]
type = "exec_tester"
[strategies.config]
strategy_id = "EXEC_TESTER-001"
instrument_id = "TOKEN.TEST"
client_id = "TEST"
order_qty = "5"
"#,
    );

    let output = Command::new(env!("CARGO_BIN_EXE_bolt-v2"))
        .args([
            "secrets",
            "resolve",
            "--config",
            path.to_str().expect("utf-8 path"),
        ])
        .output()
        .expect("secrets resolve should run");

    assert!(!output.status.success());

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Missing required secret config fields: region"));
}

#[test]
fn secrets_check_fails_when_exec_clients_are_missing() {
    let path = write_temp_config(
        r#"
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

[[strategies]]
type = "exec_tester"
[strategies.config]
strategy_id = "EXEC_TESTER-001"
instrument_id = "TOKEN.TEST"
client_id = "TEST"
order_qty = "5"
"#,
    );

    let output = Command::new(env!("CARGO_BIN_EXE_bolt-v2"))
        .args([
            "secrets",
            "check",
            "--config",
            path.to_str().expect("utf-8 path"),
        ])
        .output()
        .expect("secrets check should run");

    assert!(!output.status.success());

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Missing required config section: [[exec_clients]]"));
}

#[test]
fn run_fails_when_node_timeout_fields_are_omitted() {
    let path = write_temp_config(
        r#"
[node]
name = "BOLT-V2-001"
trader_id = "BOLT-001"
environment = "Live"
load_state = false
save_state = false
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
"#,
    );

    let output = Command::new(env!("CARGO_BIN_EXE_bolt-v2"))
        .args(["run", "--config", path.to_str().expect("utf-8 path")])
        .output()
        .expect("run should validate config before startup");

    assert!(!output.status.success());

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Config field node.timeout_connection_secs must be greater than 0"));
}

#[test]
fn run_allows_zero_delays_and_reaches_secret_validation() {
    let path = write_temp_config(
        r#"
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
delay_post_stop_secs = 0
delay_shutdown_secs = 0

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
"#,
    );

    let output = Command::new(env!("CARGO_BIN_EXE_bolt-v2"))
        .args(["run", "--config", path.to_str().expect("utf-8 path")])
        .output()
        .expect("run should validate config before startup");

    assert!(!output.status.success());

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr
            .contains("Missing required secret config fields: pk, api_key, api_secret, passphrase")
    );
}

fn write_temp_config(contents: &str) -> std::path::PathBuf {
    let timestamp_nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should move forward")
        .as_nanos();
    let path = temp_config_path_for_timestamp(timestamp_nanos);
    fs::write(&path, contents).expect("temp config should be written");
    path
}

fn temp_config_path_for_timestamp(timestamp_nanos: u128) -> std::path::PathBuf {
    let counter = TEMP_CONFIG_COUNTER.fetch_add(1, Ordering::Relaxed);
    let filename = format!("bolt-v2-cli-{timestamp_nanos}-{counter}.toml");
    std::env::temp_dir().join(filename)
}
