use std::{fs, process::Command};

use bolt_v2::{
    bolt_v3_config::{LiveCanaryOperatorEvidenceBlock, load_bolt_v3_config},
    bolt_v3_operator_artifacts::compute_operator_approval_envelope_sha256,
};

mod support;
use support::{repo_path, valid_live_canary_operator_evidence};
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
fn bolt_v3_cli_exposes_final_operator_packet_verifier_command() {
    let output = Command::new(env!("CARGO_BIN_EXE_bolt-v2"))
        .args(["operator-artifacts", "verify-final", "--help"])
        .output()
        .expect("bolt-v3 final operator packet verifier help should run");

    assert!(
        output.status.success(),
        "expected operator-artifacts verify-final help to pass, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("--config"));
    assert!(stdout.contains("--operator-packet"));
    assert!(stdout.contains("--verification-stage"));
}

#[test]
fn bolt_v3_cli_exposes_final_operator_packet_assembly_command() {
    let output = Command::new(env!("CARGO_BIN_EXE_bolt-v2"))
        .args(["operator-artifacts", "assemble-final", "--help"])
        .output()
        .expect("bolt-v3 final operator packet assembler help should run");

    assert!(
        output.status.success(),
        "expected operator-artifacts assemble-final help to pass, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("--config"));
    assert!(stdout.contains("--static-manifest"));
    assert!(stdout.contains("--operator-packet"));
}

#[test]
fn bolt_v3_cli_exposes_static_manifest_from_operator_evidence_command() {
    let output = Command::new(env!("CARGO_BIN_EXE_bolt-v2"))
        .args([
            "operator-artifacts",
            "write-manifest-from-operator-evidence",
            "--help",
        ])
        .output()
        .expect("bolt-v3 operator-evidence manifest help should run");

    assert!(
        output.status.success(),
        "expected manifest-from-operator-evidence help to pass, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("--config"));
    assert!(stdout.contains("--output"));
}

#[test]
fn bolt_v3_cli_computes_approval_envelope_sha256_without_printing_operator_paths() {
    let operator_evidence = valid_live_canary_operator_evidence();
    let config_path = write_bolt_v3_fixture_root(|root| {
        format!(
            "{root}\n{}",
            live_canary_with_operator_evidence_toml(&operator_evidence)
        )
    });
    let output = Command::new(env!("CARGO_BIN_EXE_bolt-v2"))
        .args([
            "operator-artifacts",
            "compute-approval-envelope-sha256",
            "--config",
            config_path.to_str().expect("fixture path should be utf-8"),
        ])
        .output()
        .expect("bolt-v3 approval-envelope hash command should run");

    assert!(
        output.status.success(),
        "expected approval-envelope hash command to pass, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout_json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("stdout should be JSON");
    let loaded = load_bolt_v3_config(&config_path).expect("fixture root should load");
    let expected_sha256 = compute_operator_approval_envelope_sha256(&loaded)
        .expect("approval envelope hash should compute");
    let stdout_object = stdout_json
        .as_object()
        .expect("stdout should be a JSON object");
    assert_eq!(
        stdout_object.keys().map(String::as_str).collect::<Vec<_>>(),
        ["sha256"]
    );
    assert_eq!(stdout_json["sha256"], expected_sha256);
    let stdout = String::from_utf8_lossy(&output.stdout);
    for forbidden in [
        operator_evidence.ssm_manifest_path.as_str(),
        operator_evidence.strategy_input_evidence_path.as_str(),
        operator_evidence.financial_envelope_path.as_str(),
        operator_evidence.pre_run_state_path.as_str(),
        operator_evidence.abort_plan_path.as_str(),
        operator_evidence.approval_nonce_path.as_str(),
        operator_evidence.approval_consumption_path.as_str(),
        "operator-approved-canary-001",
    ] {
        assert!(
            !stdout.contains(forbidden),
            "stdout must not expose operator path or approval id {forbidden}"
        );
    }
}

#[test]
fn bolt_v3_cli_updates_operator_evidence_toml_without_printing_evidence_values() {
    let operator_evidence = valid_live_canary_operator_evidence();
    let config_path = write_bolt_v3_fixture_root(|root| {
        format!("{root}\n{}", live_canary_toml_without_operator_evidence())
    });
    let operator_evidence_json_path = config_path
        .parent()
        .expect("fixture root should have parent")
        .join("operator-evidence.json");
    fs::write(
        &operator_evidence_json_path,
        serde_json::to_vec_pretty(&operator_evidence)
            .expect("operator evidence JSON should encode"),
    )
    .expect("operator evidence JSON should write");

    let output = Command::new(env!("CARGO_BIN_EXE_bolt-v2"))
        .args([
            "operator-artifacts",
            "update-operator-evidence-toml",
            "--config",
            config_path.to_str().expect("fixture path should be utf-8"),
            "--operator-evidence-json",
            operator_evidence_json_path
                .to_str()
                .expect("operator evidence JSON path should be utf-8"),
            "--max-operator-evidence-json-bytes",
            "100000",
        ])
        .output()
        .expect("bolt-v3 operator-evidence TOML update command should run");

    assert!(
        output.status.success(),
        "expected operator-evidence TOML update to pass, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout_json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("stdout should be JSON");
    let stdout_object = stdout_json
        .as_object()
        .expect("stdout should be a JSON object");
    assert_eq!(
        stdout_object.keys().map(String::as_str).collect::<Vec<_>>(),
        ["root_toml_sha256"]
    );

    let loaded = load_bolt_v3_config(&config_path).expect("patched fixture root should load");
    let patched_operator_evidence = loaded
        .root
        .live_canary
        .expect("live canary should remain configured")
        .operator_evidence
        .expect("operator evidence should be patched");
    assert_eq!(patched_operator_evidence, operator_evidence);

    let stdout = String::from_utf8_lossy(&output.stdout);
    for forbidden in [
        operator_evidence.ssm_manifest_path.as_str(),
        operator_evidence.strategy_input_evidence_path.as_str(),
        operator_evidence.financial_envelope_path.as_str(),
        operator_evidence.pre_run_state_path.as_str(),
        operator_evidence.abort_plan_path.as_str(),
        operator_evidence.approval_nonce_path.as_str(),
        operator_evidence.approval_consumption_path.as_str(),
        "operator-approved-canary-001",
    ] {
        assert!(
            !stdout.contains(forbidden),
            "stdout must not expose operator evidence values or approval id {forbidden}"
        );
    }
}

#[test]
fn bolt_v3_cli_exposes_source_bundle_artifact_commands() {
    for command in [
        "generate-pre-run-state-from-source-bundle",
        "generate-abort-plan-from-source-bundle",
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_bolt-v2"))
            .args(["operator-artifacts", command, "--help"])
            .output()
            .unwrap_or_else(|error| panic!("bolt-v3 {command} help should run: {error}"));

        assert!(
            output.status.success(),
            "expected operator-artifacts {command} help to pass, stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("--config"), "{stdout}");
        assert!(stdout.contains("--strategy-instance-id"), "{stdout}");
        assert!(stdout.contains("--source-bundle"), "{stdout}");
        assert!(stdout.contains("--output"), "{stdout}");
        assert!(stdout.contains("--max-source-bundle-bytes"), "{stdout}");
    }
}

#[test]
fn bolt_v3_cli_exposes_pre_run_state_source_collector_command() {
    let output = Command::new(env!("CARGO_BIN_EXE_bolt-v2"))
        .args([
            "operator-artifacts",
            "generate-pre-run-state-from-source-collectors",
            "--help",
        ])
        .output()
        .expect("bolt-v3 source-owned pre-run-state help should run");

    assert!(
        output.status.success(),
        "expected source-owned pre-run-state help to pass, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("--config"), "{stdout}");
    assert!(stdout.contains("--strategy-instance-id"), "{stdout}");
    assert!(stdout.contains("--cargo-toml"), "{stdout}");
    assert!(stdout.contains("--cargo-lock"), "{stdout}");
    assert!(stdout.contains("--clob-signing-source"), "{stdout}");
    assert!(stdout.contains("--host-clock-source"), "{stdout}");
    assert!(stdout.contains("--venue-account-state-source"), "{stdout}");
    assert!(stdout.contains("--funding-margin-source"), "{stdout}");
    assert!(stdout.contains("--strategy-input-evidence"), "{stdout}");
    assert!(
        stdout.contains("--strategy-input-evidence-sha256"),
        "{stdout}"
    );
    assert!(
        !stdout.contains("--expected-price-to-beat-source"),
        "pre-run collector must derive the expected price-to-beat source from TOML, not a CLI value: {stdout}"
    );
    assert!(stdout.contains("--single-runner-lock"), "{stdout}");
    assert!(stdout.contains("--egress-identity-source"), "{stdout}");
    assert!(
        stdout.contains("--clob-v2-adapter-signing-source"),
        "{stdout}"
    );
    assert!(
        stdout.contains("--clob-v2-collateral-accounting-source"),
        "{stdout}"
    );
    assert!(stdout.contains("--clob-v2-fee-behavior-source"), "{stdout}");
    assert!(stdout.contains("--max-source-bytes"), "{stdout}");
    assert!(stdout.contains("--max-host-clock-skew-millis"), "{stdout}");
    assert!(
        stdout.contains("--max-single-runner-lock-bytes"),
        "{stdout}"
    );
    assert!(stdout.contains("--output"), "{stdout}");
}

#[test]
fn bolt_v3_cli_exposes_abort_plan_source_collector_command() {
    let output = Command::new(env!("CARGO_BIN_EXE_bolt-v2"))
        .args([
            "operator-artifacts",
            "generate-abort-plan-from-source-collectors",
            "--help",
        ])
        .output()
        .expect("bolt-v3 source-owned abort-plan help should run");

    assert!(
        output.status.success(),
        "expected source-owned abort-plan help to pass, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("--config"), "{stdout}");
    assert!(stdout.contains("--strategy-instance-id"), "{stdout}");
    assert!(stdout.contains("--strategy-source"), "{stdout}");
    assert!(stdout.contains("--submit-admission-source"), "{stdout}");
    assert!(stdout.contains("--max-source-bytes"), "{stdout}");
    assert!(stdout.contains("--output"), "{stdout}");
}

#[test]
fn bolt_v3_cli_exposes_strategy_input_decision_evidence_command() {
    let output = Command::new(env!("CARGO_BIN_EXE_bolt-v2"))
        .args([
            "operator-artifacts",
            "generate-strategy-input-from-decision-evidence",
            "--help",
        ])
        .output()
        .expect("bolt-v3 strategy-input decision-evidence help should run");

    assert!(
        output.status.success(),
        "expected strategy-input decision-evidence help to pass, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("--config"));
    assert!(stdout.contains("--strategy-instance-id"));
    assert!(stdout.contains("--decision-evidence"));
    assert!(stdout.contains("--max-decision-evidence-bytes"));
    assert!(stdout.contains("--market-selection-source"));
    assert!(stdout.contains("--market-selection-source-sha256"));
    assert!(stdout.contains("--candidate-market-start-timestamp-ms"));
    assert!(stdout.contains("--output"));
}

#[test]
fn bolt_v3_cli_exposes_market_selection_decision_evidence_command() {
    let output = Command::new(env!("CARGO_BIN_EXE_bolt-v2"))
        .args([
            "operator-artifacts",
            "generate-market-selection-from-decision-evidence",
            "--help",
        ])
        .output()
        .expect("bolt-v3 market-selection decision-evidence help should run");

    assert!(
        output.status.success(),
        "expected market-selection decision-evidence help to pass, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("--config"));
    assert!(stdout.contains("--strategy-instance-id"));
    assert!(stdout.contains("--decision-evidence"));
    assert!(stdout.contains("--max-decision-evidence-bytes"));
    assert!(stdout.contains("--instrument-source"));
    assert!(stdout.contains("--max-instrument-source-bytes"));
    assert!(stdout.contains("--output"));
}

#[test]
fn bolt_v3_cli_exposes_entry_decision_evidence_source_command() {
    let output = Command::new(env!("CARGO_BIN_EXE_bolt-v2"))
        .args([
            "operator-artifacts",
            "generate-entry-decision-evidence-from-source",
            "--help",
        ])
        .output()
        .expect("bolt-v3 entry decision-evidence source help should run");

    assert!(
        output.status.success(),
        "expected entry decision-evidence source help to pass, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("--config"));
    assert!(stdout.contains("--strategy-instance-id"));
    assert!(stdout.contains("--decision-source"));
    assert!(stdout.contains("--max-decision-source-bytes"));
    assert!(stdout.contains("--instrument-source"));
    assert!(stdout.contains("--max-instrument-source-bytes"));
    assert!(stdout.contains("--max-decision-evidence-bytes"));
}

#[test]
fn bolt_v3_cli_exposes_collect_entry_decision_source_inputs() {
    let output = Command::new(env!("CARGO_BIN_EXE_bolt-v2"))
        .args([
            "operator-artifacts",
            "collect-entry-decision-source-inputs",
            "--help",
        ])
        .output()
        .expect("bolt-v3 entry decision source-input collection help should run");

    assert!(
        output.status.success(),
        "expected source-input collection help to pass, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("--config"));
    assert!(stdout.contains("--strategy-instance-id"));
    assert!(stdout.contains("--price-to-beat-source"));
    assert!(stdout.contains("--max-price-to-beat-source-bytes"));
    assert!(stdout.contains("--reference-quote-source"));
    assert!(stdout.contains("--max-reference-quote-source-bytes"));
    assert!(stdout.contains("--realized-volatility-source"));
    assert!(stdout.contains("--max-realized-volatility-source-bytes"));
    assert!(stdout.contains("--fee-rate-source"));
    assert!(stdout.contains("--max-fee-rate-source-bytes"));
    assert!(stdout.contains("--decision-source-output"));
    assert!(stdout.contains("--instrument-source-output"));
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
    assert!(
        !output_dir.join("market-selection-source.json").exists(),
        "market-selection artifact must not be written without source-bound price-to-beat strategy decision evidence"
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
            .is_some_and(|blocker| blocker.contains("market-selection"))),
        "manifest should record explicit market-selection blocker: {manifest_json}"
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
        !stderr.contains("/bolt/binance_reference/api_secret"),
        "stderr must not expose failing SSM path, got: {stderr}"
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

fn live_canary_toml_without_operator_evidence() -> &'static str {
    r#"
[live_canary]
approval_id = "operator-approved-canary-001"
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
}

fn live_canary_with_operator_evidence_toml(evidence: &LiveCanaryOperatorEvidenceBlock) -> String {
    format!(
        r#"
[live_canary]
approval_id = "operator-approved-canary-001"
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

[live_canary.operator_evidence]
head_sha = "{head_sha}"
max_operator_evidence_file_bytes = {max_operator_evidence_file_bytes}
approval_consumption_max_age_seconds = {approval_consumption_max_age_seconds}
approval_envelope_path = "{approval_envelope_path}"
approval_envelope_sha256 = "{approval_envelope_sha256}"
ssm_manifest_path = "{ssm_manifest_path}"
ssm_manifest_sha256 = "{ssm_manifest_sha256}"
strategy_input_evidence_path = "{strategy_input_evidence_path}"
strategy_input_evidence_sha256 = "{strategy_input_evidence_sha256}"
financial_envelope_path = "{financial_envelope_path}"
financial_envelope_sha256 = "{financial_envelope_sha256}"
pre_run_state_path = "{pre_run_state_path}"
pre_run_state_sha256 = "{pre_run_state_sha256}"
abort_plan_path = "{abort_plan_path}"
abort_plan_sha256 = "{abort_plan_sha256}"
canary_evidence_path = "{canary_evidence_path}"
approval_not_before_unix_seconds = {approval_not_before_unix_seconds}
approval_not_after_unix_seconds = {approval_not_after_unix_seconds}
approval_nonce_path = "{approval_nonce_path}"
approval_nonce_sha256 = "{approval_nonce_sha256}"
approval_consumption_path = "{approval_consumption_path}"
decision_evidence_path = "{decision_evidence_path}"
nt_submit_event_path = "{nt_submit_event_path}"
venue_order_state_path = "{venue_order_state_path}"
strategy_cancel_path = "{strategy_cancel_path}"
restart_reconciliation_path = "{restart_reconciliation_path}"
post_run_hygiene_path = "{post_run_hygiene_path}"
"#,
        head_sha = toml_string(&evidence.head_sha),
        max_operator_evidence_file_bytes = evidence.max_operator_evidence_file_bytes,
        approval_consumption_max_age_seconds = evidence.approval_consumption_max_age_seconds,
        approval_envelope_path = toml_string(&evidence.approval_envelope_path),
        approval_envelope_sha256 = toml_string(&evidence.approval_envelope_sha256),
        ssm_manifest_path = toml_string(&evidence.ssm_manifest_path),
        ssm_manifest_sha256 = toml_string(&evidence.ssm_manifest_sha256),
        strategy_input_evidence_path = toml_string(&evidence.strategy_input_evidence_path),
        strategy_input_evidence_sha256 = toml_string(&evidence.strategy_input_evidence_sha256),
        financial_envelope_path = toml_string(&evidence.financial_envelope_path),
        financial_envelope_sha256 = toml_string(&evidence.financial_envelope_sha256),
        pre_run_state_path = toml_string(&evidence.pre_run_state_path),
        pre_run_state_sha256 = toml_string(&evidence.pre_run_state_sha256),
        abort_plan_path = toml_string(&evidence.abort_plan_path),
        abort_plan_sha256 = toml_string(&evidence.abort_plan_sha256),
        canary_evidence_path = toml_string(&evidence.canary_evidence_path),
        approval_not_before_unix_seconds = evidence.approval_not_before_unix_seconds,
        approval_not_after_unix_seconds = evidence.approval_not_after_unix_seconds,
        approval_nonce_path = toml_string(&evidence.approval_nonce_path),
        approval_nonce_sha256 = toml_string(&evidence.approval_nonce_sha256),
        approval_consumption_path = toml_string(&evidence.approval_consumption_path),
        decision_evidence_path = toml_string(&evidence.decision_evidence_path),
        nt_submit_event_path = toml_string(&evidence.nt_submit_event_path),
        venue_order_state_path = toml_string(&evidence.venue_order_state_path),
        strategy_cancel_path = toml_string(
            evidence
                .strategy_cancel_path
                .as_deref()
                .expect("test operator evidence should include strategy cancel path"),
        ),
        restart_reconciliation_path = toml_string(&evidence.restart_reconciliation_path),
        post_run_hygiene_path = toml_string(&evidence.post_run_hygiene_path),
    )
}

fn toml_string(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}
