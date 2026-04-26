use std::{fs, process::Command};

use bolt_v2::materialize_live_config;
mod support;
use support::repo_path;
use tempfile::{NamedTempFile, tempdir};

fn assert_runtime_validation_failed(stderr: &str, expected: &str) {
    assert!(
        stderr.contains("Runtime config validation failed"),
        "expected runtime validation failure, got: {stderr}"
    );
    assert!(
        stderr.contains(expected),
        "expected runtime validation failure to mention {expected:?}, got: {stderr}"
    );
}

#[test]
fn temp_config_paths_remain_unique() {
    let first = new_temp_config_path();
    let second = new_temp_config_path();

    assert_ne!(first, second);
}

#[test]
fn secrets_check_reports_complete_secret_config() {
    let path = write_generated_runtime_config();
    let output = Command::new(env!("CARGO_BIN_EXE_bolt-v2"))
        .args([
            "secrets",
            "check",
            "--config",
            path.to_str().expect("utf-8 path"),
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
fn secrets_check_fails_when_runtime_has_no_active_path() {
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
event_slugs = ["btc-updown-5m"]

[[exec_clients]]
name = "TEST"
type = "polymarket"
[exec_clients.config]
account_id = "TEST-001"
signature_type = 2
funder = "0xabc"
[exec_clients.secrets]
region = "eu-west-1"
pk = "/x/pk"
api_key = "/x/key"
api_secret = "/x/secret"
passphrase = "/x/pass"

[raw_capture]
output_dir = "/srv/bolt-v2/var/raw"
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
        stderr.contains("Runtime config must enable at least one ruleset or strategy"),
        "expected active-runtime-path error, got: {stderr}"
    );
}

#[test]
fn secrets_check_fails_on_invalid_config_via_load_validation() {
    let path = write_temp_config(
        r#"
[node]
name = "BOLT-V2-001"
trader_id = "BOLT001"
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
event_slugs = ["btc-updown-5m"]

[[exec_clients]]
name = "TEST"
type = "polymarket"
[exec_clients.config]
account_id = "TEST-001"
signature_type = 2
funder = "0xabc"
[exec_clients.secrets]
region = "eu-west-1"
pk = "/x/pk"
api_key = "/x/key"
api_secret = "/x/secret"
passphrase = "/x/pass"
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
    assert_runtime_validation_failed(&stderr, "node.trader_id");
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
event_slugs = ["btc-updown-5m"]

[[exec_clients]]
name = "TEST"
type = "polymarket"
[exec_clients.config]
account_id = "TEST-001"
signature_type = 2
funder = "0xabc"
[exec_clients.secrets]
region = "eu-west-1"
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
    assert_runtime_validation_failed(&stderr, "exec_clients[0].secrets.pk");
    assert_runtime_validation_failed(&stderr, "exec_clients[0].secrets.api_key");
    assert_runtime_validation_failed(&stderr, "exec_clients[0].secrets.api_secret");
    assert_runtime_validation_failed(&stderr, "exec_clients[0].secrets.passphrase");
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
event_slugs = ["btc-updown-5m"]

[[exec_clients]]
name = "TEST"
type = "polymarket"
[exec_clients.config]
account_id = "TEST-001"
signature_type = 2
funder = "0xabc"
[exec_clients.secrets]
region = "eu-west-1"
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
    assert_runtime_validation_failed(&stderr, "exec_clients[0].secrets.pk");
    assert_runtime_validation_failed(&stderr, "exec_clients[0].secrets.api_key");
    assert_runtime_validation_failed(&stderr, "exec_clients[0].secrets.api_secret");
    assert_runtime_validation_failed(&stderr, "exec_clients[0].secrets.passphrase");
}

#[test]
fn secrets_resolve_surfaces_binance_ssm_failure() {
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
name = "TESTDATA"
type = "polymarket"
[data_clients.config]
gamma_event_fetch_max_concurrent = 8

[[exec_clients]]
name = "TESTEXEC"
type = "polymarket"
[exec_clients.config]
account_id = "POLYMARKET-001"
signature_type = 2
funder = "0xabc"
[exec_clients.secrets]
region = "us-east-1"
pk = "/bolt/poly/pk"
api_key = "/bolt/poly/api-key"
api_secret = "/bolt/poly/api-secret"
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
instrument_status_poll_secs = 0

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
"#,
    );

    // The bolt-v2 secrets resolution path now uses the AWS Rust SDK
    // (`aws-sdk-ssm`) directly, not a subprocess `aws` CLI, so the
    // previous fake-script-on-PATH mock is no longer reachable from
    // production code. Reserve a localhost ephemeral port and drop
    // the listener so any TCP connection to that port is refused
    // immediately; pointing the SDK's SSM endpoint at it forces every
    // GetParameter call to fail at the dispatch layer without
    // touching real AWS.
    let listener = std::net::TcpListener::bind("127.0.0.1:0")
        .expect("ephemeral port reservation should succeed");
    let unused_port = listener
        .local_addr()
        .expect("local addr should be readable")
        .port();
    drop(listener);
    let unreachable_endpoint = format!("http://127.0.0.1:{unused_port}");

    let output = Command::new(env!("CARGO_BIN_EXE_bolt-v2"))
        .args([
            "secrets",
            "resolve",
            "--config",
            path.to_str().expect("utf-8 path"),
        ])
        // SDK endpoint override for SSM only; leaves other AWS
        // services unaffected.
        .env("AWS_ENDPOINT_URL_SSM", &unreachable_endpoint)
        // Bogus credentials short-circuit the credential provider
        // chain so the SDK does not query IMDS, profiles, or SSO and
        // the test stays fully offline. The connection to
        // `unreachable_endpoint` fails before any signed request
        // would matter.
        .env("AWS_ACCESS_KEY_ID", "fake-access-key")
        .env("AWS_SECRET_ACCESS_KEY", "fake-secret-key")
        .env("AWS_REGION", "us-east-1")
        // Cap retries so the test does not pay for the SDK's default
        // exponential backoff against a deliberately refused port.
        .env("AWS_MAX_ATTEMPTS", "1")
        .output()
        .expect("secrets resolve should run");

    assert!(!output.status.success());

    let stderr = String::from_utf8_lossy(&output.stderr);
    // `secrets::resolve_binance_with` resolves api_secret first so
    // failures localize to unusable key material immediately, so the
    // first SSM call surfaced when binance reference is configured
    // is `/bolt/binance/api-secret`.
    assert!(
        stderr.contains("/bolt/binance/api-secret"),
        "expected failing Binance SSM path in stderr, got: {stderr}"
    );
    assert!(
        stderr.contains("AWS SSM GetParameter failed"),
        "expected production SSM SDK error context in stderr, got: {stderr}"
    );
}

#[test]
fn secrets_resolve_fails_when_runtime_has_no_active_path() {
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
event_slugs = ["btc-updown-5m"]

[[exec_clients]]
name = "TEST"
type = "polymarket"
[exec_clients.config]
account_id = "TEST-001"
signature_type = 2
funder = "0xabc"
[exec_clients.secrets]
region = "eu-west-1"
pk = "/x/pk"
api_key = "/x/key"
api_secret = "/x/secret"
passphrase = "/x/pass"

[raw_capture]
output_dir = "/srv/bolt-v2/var/raw"
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
        stderr.contains("Runtime config must enable at least one ruleset or strategy"),
        "expected active-runtime-path error, got: {stderr}"
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
event_slugs = ["btc-updown-5m"]

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
    assert_runtime_validation_failed(&stderr, "exec_clients[0].secrets.region");
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
event_slugs = ["btc-updown-5m"]

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
    assert_runtime_validation_failed(&stderr, "exec_clients[0].secrets.region");
}

#[test]
fn stream_to_lake_fails_when_live_spool_is_missing() {
    let source_root = tempdir().unwrap();
    let output_root = tempdir().unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_stream_to_lake"))
        .args([
            "--catalog-path",
            source_root.path().to_str().expect("utf-8 path"),
            "--instance-id",
            "missing-instance",
            "--output-root",
            output_root.path().to_str().expect("utf-8 path"),
        ])
        .output()
        .expect("stream_to_lake should run");

    assert!(!output.status.success());

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("missing live spool instance directory"),
        "{stderr}"
    );
}

#[test]
fn stream_to_lake_rejects_relative_contract_path() {
    let source_root = tempdir().unwrap();
    let output_root = tempdir().unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_stream_to_lake"))
        .args([
            "--catalog-path",
            source_root.path().to_str().expect("utf-8 path"),
            "--instance-id",
            "missing-instance",
            "--output-root",
            output_root.path().to_str().expect("utf-8 path"),
            "--contract",
            "contracts/polymarket.toml",
        ])
        .output()
        .expect("stream_to_lake should run");

    assert!(!output.status.success());

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("contract_path must be a local absolute path"),
        "{stderr}"
    );
}

fn write_temp_config(contents: &str) -> std::path::PathBuf {
    let path = new_temp_config_path();
    fs::write(&path, contents).expect("temp config should be written");
    path
}

fn write_generated_runtime_config() -> std::path::PathBuf {
    let path = new_temp_config_path();
    materialize_live_config(&repo_path("config/live.local.example.toml"), &path)
        .expect("tracked template should materialize");
    path
}

fn new_temp_config_path() -> std::path::PathBuf {
    let path = NamedTempFile::new()
        .expect("temp config file should create")
        .into_temp_path()
        .keep()
        .expect("temp config path should persist");
    fs::remove_file(&path).expect("temp config placeholder should remove");
    path
}
