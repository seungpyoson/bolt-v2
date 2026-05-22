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
            "clients.polymarket_main: required secret fields present \
             (private_key_ssm_path, api_key_ssm_path, api_secret_ssm_path, passphrase_ssm_path)"
        ),
        "expected Polymarket secret field inventory, got: {stdout}"
    );
    assert!(
        stdout.contains(
            "clients.binance_reference: required secret fields present \
             (api_key_ssm_path, api_secret_ssm_path)"
        ),
        "expected Binance secret field inventory, got: {stdout}"
    );
}

#[test]
fn bolt_v3_cli_exposes_no_submit_readiness_operator_command() {
    let output = Command::new(env!("CARGO_BIN_EXE_bolt-v2"))
        .args(["no-submit-readiness", "--help"])
        .output()
        .expect("bolt-v3 no-submit readiness help should run");

    assert!(
        output.status.success(),
        "expected no-submit-readiness help to pass, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("--config"));
}

#[test]
fn bolt_v3_cli_exposes_static_operator_artifacts_command() {
    let output = Command::new(env!("CARGO_BIN_EXE_bolt-v2"))
        .args(["operator-artifacts", "generate-static", "--help"])
        .output()
        .expect("bolt-v3 static operator artifacts help should run");

    assert!(
        output.status.success(),
        "expected operator-artifacts generate-static help to pass, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("--config"));
    assert!(stdout.contains("--output-dir"));
    assert!(stdout.contains("--strategy-instance-id"));
}

#[test]
fn bolt_v3_static_operator_artifacts_command_fails_closed_on_abort_blocker() {
    let config_path = write_bolt_v3_fixture_root(|root| {
        format!(
            "{root}\n{}",
            r#"
[live_canary]
approval_id = "test-operator-approval"
no_submit_readiness_report_path = "reports/no-submit-readiness.json"
max_no_submit_readiness_report_bytes = 1000000
readiness_report_max_age_seconds = 300
reference_quote_max_age_seconds = 30
reference_quote_wait_timeout_seconds = 5
reference_quote_probe_actor_id = "test-reference-probe"
reference_quote_probe_log_events = false
reference_quote_probe_log_commands = false
max_live_order_count = 1
max_notional_per_order = "10.00"
"#
        )
    });
    let output_dir = tempdir().expect("tempdir should create").keep();
    let output = Command::new(env!("CARGO_BIN_EXE_bolt-v2"))
        .args([
            "operator-artifacts",
            "generate-static",
            "--config",
            config_path.to_str().expect("fixture path should be utf-8"),
            "--output-dir",
            output_dir.to_str().expect("output path should be utf-8"),
            "--strategy-instance-id",
            "bitcoin_updown_main",
        ])
        .output()
        .expect("bolt-v3 static operator artifacts command should run");

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("panic gate and service policy"),
        "expected real abort blocker, got: {stderr}"
    );
    assert!(
        stderr.contains("T046 remains blocked"),
        "expected explicit T046 strategy-input blocker, got: {stderr}"
    );
    assert!(
        stderr.contains("pre-run state"),
        "expected explicit pre-run state blocker, got: {stderr}"
    );
    assert!(
        stderr.contains("T121 remains blocked"),
        "expected explicit T121 pre-run state blocker, got: {stderr}"
    );
    for artifact_name in [
        "ssm-manifest.json",
        "financial-envelope.json",
        "approval-nonce.json",
        "static-artifacts-manifest.json",
    ] {
        assert!(
            output_dir.join(artifact_name).exists(),
            "fail-closed command should still write accepted static artifact {artifact_name}"
        );
        assert!(
            stdout.contains(artifact_name),
            "stdout should report generated artifact {artifact_name}: {stdout}"
        );
    }
    for forbidden in [
        "/bolt/polymarket_main/private_key",
        "/bolt/polymarket_main/api_key",
        "/bolt/polymarket_main/api_secret",
        "/bolt/polymarket_main/passphrase",
        "/bolt/binance_reference/api_key",
        "/bolt/binance_reference/api_secret",
        "nonce_bytes",
        "nonce_material",
    ] {
        assert!(
            !stdout.contains(forbidden),
            "stdout must not expose raw secret path or nonce material {forbidden}"
        );
    }
    assert!(
        !output_dir.join("strategy-input.json").exists(),
        "strategy-input artifact must not be written without source-bound strategy decision evidence"
    );
    assert!(
        !output_dir.join("pre-run-state.json").exists(),
        "pre-run state artifact must not be written without source-bound pre-run evidence"
    );
    let manifest_path = output_dir.join("static-artifacts-manifest.json");
    let stdout_json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("stdout summary should be JSON");
    let stdout_summary = stdout_json
        .as_object()
        .expect("stdout summary should be an object");
    assert_eq!(
        stdout_summary
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["generated_artifacts", "manifest_artifact"]
    );
    for artifact in stdout_json["generated_artifacts"]
        .as_array()
        .expect("stdout generated artifacts should be an array")
        .iter()
        .chain(std::iter::once(&stdout_json["manifest_artifact"]))
    {
        let artifact = artifact
            .as_object()
            .expect("stdout artifact ref should be an object");
        assert_eq!(
            artifact.keys().map(String::as_str).collect::<Vec<_>>(),
            ["path", "sha256"]
        );
    }
    let manifest_json: serde_json::Value = serde_json::from_slice(
        &fs::read(&manifest_path).expect("static artifact manifest should read"),
    )
    .expect("static artifact manifest should parse");
    assert_eq!(
        manifest_json["record_kind"],
        "bolt_v3.static_operator_artifacts_manifest.v1"
    );
    let generated = manifest_json["generated_artifacts"]
        .as_array()
        .expect("generated artifacts should be an array");
    for artifact_name in ["ssm-manifest", "financial-envelope", "approval-nonce"] {
        let artifact = generated
            .iter()
            .find(|artifact| artifact["name"] == artifact_name)
            .unwrap_or_else(|| panic!("manifest should list {artifact_name}"));
        assert_eq!(
            artifact["sha256"]
                .as_str()
                .expect("artifact sha should be a string")
                .len(),
            64
        );
    }
    let blockers = manifest_json["blockers"]
        .as_array()
        .expect("blockers should be an array");
    assert!(
        blockers
            .iter()
            .any(|blocker| blocker == "panic gate and service policy"),
        "manifest should record abort blocker: {manifest_json}"
    );
    assert!(
        blockers.iter().any(|blocker| blocker
            .as_str()
            .is_some_and(|blocker| blocker.contains("T046 remains blocked"))),
        "manifest should record explicit strategy-input blocker: {manifest_json}"
    );
    assert!(
        blockers.iter().any(|blocker| blocker
            .as_str()
            .is_some_and(|blocker| blocker.contains("pre-run state"))),
        "manifest should record explicit pre-run state blocker: {manifest_json}"
    );
    assert!(
        blockers.iter().any(|blocker| blocker
            .as_str()
            .is_some_and(|blocker| blocker.contains("T121 remains blocked"))),
        "manifest should record explicit T121 blocker: {manifest_json}"
    );
    assert!(
        !output_dir.join("abort-plan.json").exists(),
        "fail-closed command must not write successful abort plan"
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
    assert!(stderr.contains("clients.polymarket_main.secrets:"));
    assert!(stderr.contains("api_secret_ssm_path"));
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
