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
