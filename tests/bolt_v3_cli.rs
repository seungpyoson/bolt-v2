use std::{fs, process::Command};

mod support;
use support::repo_path;
use tempfile::tempdir;

#[test]
fn bolt_v3_secrets_check_reports_provider_secret_fields() {
    let config_path = repo_path("tests/fixtures/bolt_v3/root.toml");
    let output = Command::new(env!("CARGO_BIN_EXE_bolt-v2"))
        .args([
            "secrets",
            "check",
            "--config",
            config_path.to_str().expect("fixture path should be utf-8"),
        ])
        .output()
        .expect("bolt-v3 secrets check should run");

    assert!(
        output.status.success(),
        "expected bolt-v3 secrets check to pass, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains(
            "venues.polymarket_main: secret config complete \
             (private_key_ssm_path, api_key_ssm_path, api_secret_ssm_path, passphrase_ssm_path)"
        ),
        "expected Polymarket secret field inventory, got: {stdout}"
    );
    assert!(
        stdout.contains(
            "venues.binance_reference: secret config complete \
             (api_key_ssm_path, api_secret_ssm_path)"
        ),
        "expected Binance secret field inventory, got: {stdout}"
    );
}

#[test]
fn bolt_v3_secrets_check_rejects_missing_provider_secret_field() {
    let config_path = write_bolt_v3_fixture_root(|root| {
        root.replace(
            "api_secret_ssm_path = \"/bolt/polymarket_main/api_secret\"\n",
            "",
        )
    });
    let output = Command::new(env!("CARGO_BIN_EXE_bolt-v2"))
        .args([
            "secrets",
            "check",
            "--config",
            config_path.to_str().expect("fixture path should be utf-8"),
        ])
        .output()
        .expect("bolt-v3 secrets check should run");

    assert!(!output.status.success());

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("venues.polymarket_main.secrets: missing field `api_secret_ssm_path`"),
        "expected missing provider secret field, got: {stderr}"
    );
}

#[test]
fn bolt_v3_secrets_resolve_surfaces_ssm_failure() {
    let config_path = repo_path("tests/fixtures/bolt_v3/root.toml");
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
            config_path.to_str().expect("fixture path should be utf-8"),
        ])
        .env("AWS_ENDPOINT_URL_SSM", &unreachable_endpoint)
        .env("AWS_ACCESS_KEY_ID", "fake-access-key")
        .env("AWS_SECRET_ACCESS_KEY", "fake-secret-key")
        .env("AWS_REGION", "eu-west-1")
        .env("AWS_MAX_ATTEMPTS", "1")
        .output()
        .expect("bolt-v3 secrets resolve should run");

    assert!(!output.status.success());

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("/bolt/binance_reference/api_secret"),
        "expected failing Binance SSM path in stderr, got: {stderr}"
    );
    assert!(
        stderr.contains("AWS SSM GetParameter failed"),
        "expected production SSM SDK error context in stderr, got: {stderr}"
    );
}

fn write_bolt_v3_fixture_root<F>(mut rewrite: F) -> std::path::PathBuf
where
    F: FnMut(&str) -> String,
{
    let dir = tempdir().expect("tempdir should create").keep();
    let strategy_dir = dir.join("strategies");
    fs::create_dir_all(&strategy_dir).expect("strategy fixture dir should create");
    fs::write(
        strategy_dir.join("binary_oracle.toml"),
        include_str!("fixtures/bolt_v3/strategies/binary_oracle.toml"),
    )
    .expect("strategy fixture should write");
    let root_path = dir.join("root.toml");
    fs::write(
        &root_path,
        rewrite(include_str!("fixtures/bolt_v3/root.toml")),
    )
    .expect("root fixture should write");
    root_path
}
