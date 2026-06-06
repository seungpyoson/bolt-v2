use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Write},
    net::TcpListener,
    path::Path,
    process::{Command, Output},
    sync::mpsc,
    thread,
    time::Duration,
};

use bolt_v2::{
    bolt_v3_config::{LiveCanaryOperatorEvidenceBlock, load_bolt_v3_config},
    bolt_v3_operator_artifacts::compute_operator_approval_envelope_sha256,
};
use nautilus_polymarket::{
    common::consts::DUST_POSITION_THRESHOLD,
    signing::eip712::{CTF_EXCHANGE, NEG_RISK_CTF_EXCHANGE},
};
use sha2::{Digest, Sha256};

mod support;
use support::{
    repo_path, valid_entry_readiness_gate_session_json, valid_live_canary_operator_evidence,
};
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
fn bolt_v3_cli_exposes_live_submit_approval_artifact_command_without_raw_secret_inputs() {
    let output = Command::new(env!("CARGO_BIN_EXE_bolt-v2"))
        .args([
            "operator-artifacts",
            "generate-live-submit-approval",
            "--help",
        ])
        .output()
        .expect("bolt-v3 live-submit approval help should run");

    assert!(
        output.status.success(),
        "expected operator-artifacts generate-live-submit-approval help to pass, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("--config"), "{stdout}");
    assert!(stdout.contains("--client-key"), "{stdout}");
    assert!(stdout.contains("--expires-at-unix-seconds"), "{stdout}");
    assert!(
        !stdout.contains("--private-key") && !stdout.contains("--account-address"),
        "live-submit approval materialization must derive signer identity from configured SSM secrets: {stdout}"
    );
}

#[test]
fn bolt_v3_cli_exposes_live_submit_arming_preflight_without_raw_secret_inputs() {
    let output = Command::new(env!("CARGO_BIN_EXE_bolt-v2"))
        .args([
            "operator-artifacts",
            "preflight-live-submit-arming",
            "--help",
        ])
        .output()
        .expect("bolt-v3 live-submit arming preflight help should run");

    assert!(
        output.status.success(),
        "expected operator-artifacts preflight-live-submit-arming help to pass, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("--config"), "{stdout}");
    assert!(stdout.contains("--client-key"), "{stdout}");
    assert!(
        !stdout.contains("--expires-at-unix-seconds"),
        "preflight must not materialize a new approval artifact: {stdout}"
    );
    assert!(
        !stdout.contains("--private-key")
            && !stdout.contains("--account-address")
            && !stdout.contains("--approval-id"),
        "live-submit arming preflight must derive signer and approval identity from configured SSM/TOML: {stdout}"
    );
}

#[test]
fn bolt_v3_cli_exposes_hyperliquid_product_submit_proof_command_without_raw_secret_inputs() {
    let output = Command::new(env!("CARGO_BIN_EXE_bolt-v2"))
        .args([
            "operator-artifacts",
            "generate-product-submit-proof",
            "--help",
        ])
        .output()
        .expect("bolt-v3 Hyperliquid product-submit proof help should run");

    assert!(
        output.status.success(),
        "expected operator-artifacts generate-product-submit-proof help to pass, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("--provider-key"), "{stdout}");
    assert!(stdout.contains("--provider-id"), "{stdout}");
    assert!(stdout.contains("--product-surface"), "{stdout}");
    assert!(stdout.contains("--toml-checksum"), "{stdout}");
    assert!(stdout.contains("--order-proof-artifact-path"), "{stdout}");
    assert!(stdout.contains("--order-proof-artifact-sha256"), "{stdout}");
    assert!(stdout.contains("--fill-proof-artifact-path"), "{stdout}");
    assert!(stdout.contains("--fill-proof-artifact-sha256"), "{stdout}");
    assert!(
        stdout.contains("--rounding-proof-artifact-path"),
        "{stdout}"
    );
    assert!(
        stdout.contains("--rounding-proof-artifact-sha256"),
        "{stdout}"
    );
    assert!(stdout.contains("--fee-proof-artifact-path"), "{stdout}");
    assert!(stdout.contains("--fee-proof-artifact-sha256"), "{stdout}");
    assert!(
        stdout.contains("--settlement-proof-artifact-path"),
        "{stdout}"
    );
    assert!(
        stdout.contains("--settlement-proof-artifact-sha256"),
        "{stdout}"
    );
    assert!(stdout.contains("--output"), "{stdout}");
    assert!(
        !stdout.contains("--private-key") && !stdout.contains("--account-address"),
        "product-submit proof materialization must not accept raw signer secrets: {stdout}"
    );
}

#[test]
fn bolt_v3_cli_writes_hyperliquid_product_submit_proof_artifact() {
    let temp = tempdir().expect("temp dir should create");
    let output_path = temp.path().join("hyperliquid-product-submit-proof.json");
    let toml_checksum = "b".repeat(64);
    let order_proof_sha256 = "e".repeat(64);
    let fill_proof_sha256 = "f".repeat(64);
    let rounding_proof_sha256 = "a".repeat(64);
    let fee_proof_sha256 = "c".repeat(64);
    let output = Command::new(env!("CARGO_BIN_EXE_bolt-v2"))
        .args([
            "operator-artifacts",
            "generate-product-submit-proof",
            "--provider-key",
            "HYPERLIQUID",
            "--provider-id",
            "hyperliquid-standard-perps-test",
            "--product-surface",
            "standard_perps",
            "--toml-checksum",
            &toml_checksum,
            "--order-proof-artifact-path",
            "operator/order-proof.json",
            "--order-proof-artifact-sha256",
            &order_proof_sha256,
            "--fill-proof-artifact-path",
            "operator/fill-proof.json",
            "--fill-proof-artifact-sha256",
            &fill_proof_sha256,
            "--rounding-proof-artifact-path",
            "operator/rounding-proof.json",
            "--rounding-proof-artifact-sha256",
            &rounding_proof_sha256,
            "--fee-proof-artifact-path",
            "operator/fee-proof.json",
            "--fee-proof-artifact-sha256",
            &fee_proof_sha256,
            "--output",
            output_path
                .to_str()
                .expect("product proof output path should be utf-8"),
        ])
        .output()
        .expect("Hyperliquid product-submit proof command should run");

    assert!(
        output.status.success(),
        "expected product-submit proof command to pass, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("command stdout should be JSON");
    assert_eq!(
        stdout["path"],
        output_path
            .to_str()
            .expect("product proof output path should be utf-8")
    );
    assert_eq!(stdout["sha256"], sha256_file_for_cli_test(&output_path));

    let artifact: serde_json::Value = serde_json::from_slice(
        &fs::read(&output_path).expect("product proof artifact should read"),
    )
    .expect("product proof artifact should parse");
    assert_eq!(
        artifact["record_kind"],
        "bolt_v3.hyperliquid_product_submit_proof.v1"
    );
    assert_eq!(artifact["provider_key"], "HYPERLIQUID");
    assert_eq!(artifact["product_surface"], "standard_perps");
    assert!(artifact["settlement_proof"].is_null());
}

#[test]
fn bolt_v3_cli_exposes_base_static_operator_artifacts_command() {
    let output = Command::new(env!("CARGO_BIN_EXE_bolt-v2"))
        .args(["operator-artifacts", "generate-base-static", "--help"])
        .output()
        .expect("bolt-v3 base static operator artifacts help should run");

    assert!(
        output.status.success(),
        "expected operator-artifacts generate-base-static help to pass, stderr: {}",
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
fn bolt_v3_cli_generates_operator_evidence_json_without_printing_values() {
    let config_path = write_bolt_v3_fixture_root(|root| {
        format!("{root}\n{}", live_canary_toml_without_operator_evidence())
    });
    let evidence_dir = config_path
        .parent()
        .expect("fixture root should have parent")
        .join("operator-evidence");
    fs::create_dir_all(&evidence_dir).expect("operator evidence dir should create");
    // The materializer seals the no-submit readiness-report file hash into the
    // approval envelope, so the report referenced by [live_canary] must exist
    // under the config root before the command runs. The fixture TOML sets
    // `no_submit_readiness_report_path = "reports/no-submit-readiness.json"`,
    // resolved relative to the config root's parent.
    let config_root_parent = config_path
        .parent()
        .expect("fixture root should have parent");
    let readiness_report_path = config_root_parent
        .join("reports")
        .join("no-submit-readiness.json");
    fs::create_dir_all(
        readiness_report_path
            .parent()
            .expect("readiness report path should have parent"),
    )
    .expect("readiness report dir should create");
    fs::write(
        &readiness_report_path,
        serde_json::to_vec_pretty(&serde_json::json!({"record_kind": "test_no_submit_readiness"}))
            .expect("readiness report fixture should encode"),
    )
    .expect("readiness report fixture should write");
    let output_path = evidence_dir.join("operator-evidence.json");
    let approval_envelope_path = evidence_dir.join("approval-envelope.json");
    let ssm_manifest_path = write_cli_json_artifact(
        &evidence_dir,
        "ssm-manifest.json",
        serde_json::json!({"record_kind": "test_ssm_manifest"}),
    );
    let strategy_input_path = write_cli_json_artifact(
        &evidence_dir,
        "strategy-input.json",
        serde_json::json!({"record_kind": "test_strategy_input"}),
    );
    let gate_session_path = write_cli_json_artifact(
        &evidence_dir,
        "entry-readiness-gate-session.json",
        valid_entry_readiness_gate_session_json(),
    );
    let expected_gate_session_sha256 = sha256_file_for_cli_test(&gate_session_path);
    let financial_envelope_path = write_cli_json_artifact(
        &evidence_dir,
        "financial-envelope.json",
        serde_json::json!({"record_kind": "test_financial_envelope"}),
    );
    let pre_run_state_path = write_cli_json_artifact(
        &evidence_dir,
        "pre-run-state.json",
        serde_json::json!({"record_kind": "test_pre_run_state"}),
    );
    let abort_plan_path = write_cli_json_artifact(
        &evidence_dir,
        "abort-plan.json",
        serde_json::json!({"record_kind": "test_abort_plan"}),
    );
    let canary_proof_candidate_source_path = write_cli_json_artifact(
        &evidence_dir,
        "canary-proof-candidate-source.json",
        serde_json::json!({"record_kind": "bolt_v3_canary_proof_candidate_source"}),
    );
    let canary_proof_order_intent_path = write_cli_json_artifact(
        &evidence_dir,
        "canary-proof-order-intent.json",
        serde_json::json!({"record_kind": "bolt_v3_canary_proof_order_intent"}),
    );
    let approval_nonce_path = write_cli_json_artifact(
        &evidence_dir,
        "approval-nonce.json",
        serde_json::json!({"record_kind": "test_approval_nonce"}),
    );
    let canary_evidence_path = evidence_dir.join("canary-evidence.json");
    let approval_consumption_path = evidence_dir.join("approval-consumed.json");
    let decision_evidence_path = evidence_dir.join("decision-evidence.jsonl");
    let nt_submit_event_path = evidence_dir.join("nt-submit-event.json");
    let venue_order_state_path = evidence_dir.join("venue-order-state.json");
    let strategy_cancel_path = evidence_dir.join("strategy-cancel.json");
    let restart_reconciliation_path = evidence_dir.join("restart-reconciliation.json");
    let post_run_hygiene_path = evidence_dir.join("post-run-hygiene.json");

    let output = Command::new(env!("CARGO_BIN_EXE_bolt-v2"))
        .args([
            "operator-artifacts",
            "generate-operator-evidence-json",
            "--config",
            config_path.to_str().expect("fixture path should be utf-8"),
            "--output",
            output_path
                .to_str()
                .expect("operator evidence output path should be utf-8"),
            "--max-operator-evidence-file-bytes",
            "4096",
            "--approval-consumption-max-age-seconds",
            "300",
            "--approval-envelope",
            approval_envelope_path
                .to_str()
                .expect("approval envelope path should be utf-8"),
            "--ssm-manifest",
            ssm_manifest_path
                .to_str()
                .expect("SSM manifest path should be utf-8"),
            "--strategy-input-evidence",
            strategy_input_path
                .to_str()
                .expect("strategy input path should be utf-8"),
            "--gate-session",
            gate_session_path
                .to_str()
                .expect("gate session path should be utf-8"),
            "--expected-gate-session-sha256",
            &expected_gate_session_sha256,
            "--financial-envelope",
            financial_envelope_path
                .to_str()
                .expect("financial envelope path should be utf-8"),
            "--pre-run-state",
            pre_run_state_path
                .to_str()
                .expect("pre-run state path should be utf-8"),
            "--abort-plan",
            abort_plan_path
                .to_str()
                .expect("abort plan path should be utf-8"),
            "--canary-proof-candidate-source",
            canary_proof_candidate_source_path
                .to_str()
                .expect("proof candidate source path should be utf-8"),
            "--canary-proof-order-intent",
            canary_proof_order_intent_path
                .to_str()
                .expect("proof order intent path should be utf-8"),
            "--canary-evidence",
            canary_evidence_path
                .to_str()
                .expect("canary evidence path should be utf-8"),
            "--approval-not-before-unix-seconds",
            "1900000000",
            "--approval-not-after-unix-seconds",
            "1900000300",
            "--approval-nonce",
            approval_nonce_path
                .to_str()
                .expect("approval nonce path should be utf-8"),
            "--approval-consumption",
            approval_consumption_path
                .to_str()
                .expect("approval consumption path should be utf-8"),
            "--decision-evidence",
            decision_evidence_path
                .to_str()
                .expect("decision evidence path should be utf-8"),
            "--nt-submit-event",
            nt_submit_event_path
                .to_str()
                .expect("NT submit event path should be utf-8"),
            "--venue-order-state",
            venue_order_state_path
                .to_str()
                .expect("venue order state path should be utf-8"),
            "--strategy-cancel",
            strategy_cancel_path
                .to_str()
                .expect("strategy cancel path should be utf-8"),
            "--restart-reconciliation",
            restart_reconciliation_path
                .to_str()
                .expect("restart reconciliation path should be utf-8"),
            "--post-run-hygiene",
            post_run_hygiene_path
                .to_str()
                .expect("post-run hygiene path should be utf-8"),
        ])
        .output()
        .expect("bolt-v3 operator evidence JSON command should run");

    assert!(
        output.status.success(),
        "expected operator evidence JSON generation to pass, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout_json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("stdout should be JSON");
    let stdout_object = stdout_json
        .as_object()
        .expect("stdout should be a JSON object");
    assert_eq!(
        stdout_object.keys().map(String::as_str).collect::<Vec<_>>(),
        ["operator_evidence_json_sha256"]
    );
    assert_eq!(
        stdout_json["operator_evidence_json_sha256"],
        sha256_file_for_cli_test(&output_path)
    );
    assert!(
        !approval_envelope_path.exists(),
        "operator evidence JSON generation must not pre-write approval-envelope"
    );

    let operator_evidence: LiveCanaryOperatorEvidenceBlock = serde_json::from_slice(
        &fs::read(&output_path).expect("operator evidence JSON should read"),
    )
    .expect("operator evidence JSON should parse");
    assert_eq!(operator_evidence.head_sha, env!("BOLT_V3_BUILD_HEAD_SHA"));
    assert_eq!(operator_evidence.max_operator_evidence_file_bytes, 4096);
    assert_eq!(operator_evidence.approval_consumption_max_age_seconds, 300);
    assert_eq!(
        operator_evidence.ssm_manifest_sha256,
        sha256_file_for_cli_test(&ssm_manifest_path)
    );
    assert_eq!(
        operator_evidence.strategy_input_evidence_sha256,
        sha256_file_for_cli_test(&strategy_input_path)
    );
    assert_eq!(
        operator_evidence.gate_session_path.as_deref(),
        Some(gate_session_path.to_str().expect("gate session path"))
    );
    assert_eq!(
        operator_evidence.expected_gate_session_sha256.as_deref(),
        Some(expected_gate_session_sha256.as_str())
    );
    assert_eq!(
        operator_evidence.financial_envelope_sha256,
        sha256_file_for_cli_test(&financial_envelope_path)
    );
    assert_eq!(
        operator_evidence.pre_run_state_sha256,
        sha256_file_for_cli_test(&pre_run_state_path)
    );
    assert_eq!(
        operator_evidence.abort_plan_sha256,
        sha256_file_for_cli_test(&abort_plan_path)
    );
    assert_eq!(
        operator_evidence
            .canary_proof_candidate_source_path
            .as_deref(),
        Some(
            canary_proof_candidate_source_path
                .to_str()
                .expect("proof candidate source path")
        )
    );
    assert_eq!(
        operator_evidence
            .canary_proof_candidate_source_sha256
            .as_deref(),
        Some(sha256_file_for_cli_test(&canary_proof_candidate_source_path).as_str())
    );
    assert_eq!(
        operator_evidence.canary_proof_order_intent_path.as_deref(),
        Some(
            canary_proof_order_intent_path
                .to_str()
                .expect("proof order intent path")
        )
    );
    assert_eq!(
        operator_evidence
            .canary_proof_order_intent_sha256
            .as_deref(),
        Some(sha256_file_for_cli_test(&canary_proof_order_intent_path).as_str())
    );
    assert_eq!(
        operator_evidence
            .no_submit_readiness_report_sha256
            .as_deref(),
        Some(sha256_file_for_cli_test(&readiness_report_path).as_str()),
        "materializer must seal the no-submit readiness-report file hash"
    );
    assert_eq!(
        operator_evidence.approval_nonce_sha256,
        sha256_file_for_cli_test(&approval_nonce_path)
    );
    assert_eq!(
        operator_evidence.strategy_cancel_path.as_deref(),
        Some(strategy_cancel_path.to_str().expect("strategy cancel path"))
    );
    let config_with_evidence = write_bolt_v3_fixture_root(|root| {
        format!(
            "{root}\n{}",
            live_canary_with_operator_evidence_toml(&operator_evidence)
        )
    });
    let loaded = load_bolt_v3_config(&config_with_evidence)
        .expect("fixture root with generated operator evidence should load");
    let expected_approval_envelope_sha256 =
        compute_operator_approval_envelope_sha256(&loaded).expect("approval hash should compute");
    assert_eq!(
        operator_evidence.approval_envelope_sha256,
        expected_approval_envelope_sha256
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    for forbidden in [
        operator_evidence.ssm_manifest_path.as_str(),
        operator_evidence.strategy_input_evidence_path.as_str(),
        operator_evidence
            .gate_session_path
            .as_deref()
            .expect("operator evidence should bind gate session path"),
        operator_evidence.financial_envelope_path.as_str(),
        operator_evidence.pre_run_state_path.as_str(),
        operator_evidence.abort_plan_path.as_str(),
        operator_evidence
            .canary_proof_candidate_source_path
            .as_deref()
            .expect("operator evidence should bind proof candidate source path"),
        operator_evidence
            .canary_proof_order_intent_path
            .as_deref()
            .expect("operator evidence should bind proof order intent path"),
        operator_evidence.approval_nonce_path.as_str(),
        "operator-approved-canary-001",
    ] {
        assert!(
            !stdout.contains(forbidden),
            "stdout must not expose operator path or approval id {forbidden}"
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
fn bolt_v3_cli_collects_host_clock_source_from_configured_provider_time() {
    let temp = tempdir().expect("tempdir should create");
    let reference_url = spawn_one_shot_date_server("Mon, 20 May 2024 12:34:56 GMT");
    let config_path = temp.path().join("root.toml");
    fs::write(
        &config_path,
        include_str!("fixtures/bolt_v3/root.toml").replacen(
            "base_url_http = \"https://clob.polymarket.com\"",
            &format!("base_url_http = \"{reference_url}\""),
            2,
        ),
    )
    .expect("test root TOML should write");
    fs::create_dir_all(temp.path().join("strategies")).expect("strategy dir should create");
    fs::write(
        temp.path().join("strategies/binary_oracle.toml"),
        include_str!("fixtures/bolt_v3/strategies/binary_oracle.toml"),
    )
    .expect("strategy TOML should write");
    let output_path = temp.path().join("host-clock-source.json");

    let output = Command::new(env!("CARGO_BIN_EXE_bolt-v2"))
        .args([
            "operator-artifacts",
            "collect-pre-run-host-clock-source",
            "--config",
            config_path.to_str().expect("config path should be utf-8"),
            "--strategy-instance-id",
            "configured_updown_main",
            "--output",
            output_path.to_str().expect("output path should be utf-8"),
        ])
        .output()
        .expect("bolt-v3 host-clock source collection should run");

    assert!(
        output.status.success(),
        "expected host-clock source collection to pass, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains("171621"),
        "stdout must not expose raw collected timestamps: {stdout}"
    );
    let json: serde_json::Value =
        serde_json::from_slice(&fs::read(&output_path).expect("host-clock source should write"))
            .expect("host-clock source should be JSON");
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["record_kind"], "bolt_v3.pre_run_host_clock_source.v1");
    assert_eq!(json["reference_unix_millis"], 1716208496000_u64);
    assert!(
        json["host_unix_millis"].as_u64().unwrap_or_default() >= 1716208496000_u64,
        "host timestamp should be collected at command runtime: {json}"
    );
}

#[test]
fn bolt_v3_cli_host_clock_source_collector_does_not_accept_caller_timestamps() {
    let output = Command::new(env!("CARGO_BIN_EXE_bolt-v2"))
        .args([
            "operator-artifacts",
            "collect-pre-run-host-clock-source",
            "--help",
        ])
        .output()
        .expect("bolt-v3 host-clock source collection help should run");

    assert!(
        output.status.success(),
        "expected host-clock source collection help to pass, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("--config"), "{stdout}");
    assert!(stdout.contains("--strategy-instance-id"), "{stdout}");
    assert!(stdout.contains("--output"), "{stdout}");
    assert!(
        !stdout.contains("--host-unix-millis") && !stdout.contains("--reference-unix-millis"),
        "host-clock source collector must not accept caller-supplied timestamps: {stdout}"
    );
}

#[test]
fn bolt_v3_cli_collects_clob_v2_adapter_signing_source_from_nt_signing_source() {
    let temp = tempdir().expect("tempdir should create");
    let clob_signing_source_path = temp.path().join("eip712.rs");
    fs::write(
        &clob_signing_source_path,
        r#"
const CLOB_AUTH_DOMAIN_VERSION: &str = "1";
const DOMAIN_NAME: &str = "Polymarket CTF Exchange";
const DOMAIN_VERSION: &str = "2";
const POLYGON_CHAIN_ID: u64 = 137;
pub const CTF_EXCHANGE: &str = "ctf";
pub const NEG_RISK_CTF_EXCHANGE: &str = "neg-risk";
struct OrderSigner;
fn sign_order() {}
fn order_hash() {}
"#,
    )
    .expect("CLOB signing source fixture should write");
    let output_path = temp.path().join("clob-v2-adapter-signing-source.json");

    let output = Command::new(env!("CARGO_BIN_EXE_bolt-v2"))
        .args([
            "operator-artifacts",
            "collect-pre-run-clob-v2-adapter-signing-source",
            "--cargo-toml",
            repo_path("Cargo.toml")
                .to_str()
                .expect("Cargo.toml path should be utf-8"),
            "--cargo-lock",
            repo_path("Cargo.lock")
                .to_str()
                .expect("Cargo.lock path should be utf-8"),
            "--clob-signing-source",
            clob_signing_source_path
                .to_str()
                .expect("CLOB signing source path should be utf-8"),
            "--max-source-bytes",
            "300000",
            "--output",
            output_path.to_str().expect("output path should be utf-8"),
        ])
        .output()
        .expect("bolt-v3 CLOB V2 adapter signing source collection should run");

    assert!(
        output.status.success(),
        "expected CLOB V2 adapter signing source collection to pass, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains("signature") && !stdout.contains("private"),
        "stdout must not expose signatures or ephemeral key material: {stdout}"
    );
    let json: serde_json::Value = serde_json::from_slice(
        &fs::read(&output_path).expect("CLOB V2 adapter signing source should write"),
    )
    .expect("CLOB V2 adapter signing source should be JSON");
    assert_eq!(json["schema_version"], 1);
    assert_eq!(
        json["record_kind"],
        "bolt_v3.pre_run_clob_v2_adapter_signing_source.v1"
    );
    assert_eq!(json["clob_signing_version"], "2");
    assert_eq!(
        json["adapter_signing_source_sha256"],
        sha256_file_for_cli_test(&clob_signing_source_path)
    );
    assert_eq!(json["signer_recovered_matches_expected"], true);
    for field in [
        "domain_requirements_sha256",
        "signed_order_fixture_sha256",
        "signature_verification_sha256",
    ] {
        let value = json[field]
            .as_str()
            .unwrap_or_else(|| panic!("{field} should be a string: {json}"));
        assert!(
            value.len() == 64
                && value
                    .chars()
                    .all(|ch| ch.is_ascii_hexdigit() && !ch.is_ascii_uppercase()),
            "{field} should be lowercase sha256 hex: {value}"
        );
    }
}

#[test]
fn bolt_v3_cli_collects_clob_v2_fee_behavior_source_from_nt_fee_sources() {
    let temp = tempdir().expect("tempdir should create");
    let execution_parse_source_path = temp.path().join("execution-parse.rs");
    fs::write(
        &execution_parse_source_path,
        r#"
pub fn instrument_taker_fee() {}
pub fn adjust_market_buy_amount() {}
pub fn compute_commission() {}
LiquiditySide::Taker;
Decimal::ONE - price;
price <= Decimal::ZERO || price >= Decimal::ONE;
"#,
    )
    .expect("execution fee source fixture should write");
    let http_parse_source_path = temp.path().join("http-parse.rs");
    fs::write(
        &http_parse_source_path,
        r#"
let maker_fee: Option<Decimal> = market.fee_schedule.as_ref().map(|_| Decimal::ZERO);
let taker_fee: Option<Decimal> = market.fee_schedule.as_ref().and_then(|fs| Decimal::try_from(fs.rate).ok());
feeSchedule;
"#,
    )
    .expect("HTTP fee source fixture should write");
    let output_path = temp.path().join("clob-v2-fee-behavior-source.json");

    let output = Command::new(env!("CARGO_BIN_EXE_bolt-v2"))
        .args([
            "operator-artifacts",
            "collect-pre-run-clob-v2-fee-behavior-source",
            "--nt-execution-parse-source",
            execution_parse_source_path
                .to_str()
                .expect("execution parse source path should be utf-8"),
            "--nt-http-parse-source",
            http_parse_source_path
                .to_str()
                .expect("HTTP parse source path should be utf-8"),
            "--max-source-bytes",
            "300000",
            "--output",
            output_path.to_str().expect("output path should be utf-8"),
        ])
        .output()
        .expect("bolt-v3 CLOB V2 fee behavior source collection should run");

    assert!(
        output.status.success(),
        "expected CLOB V2 fee behavior source collection to pass, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains("compute_commission") && !stdout.contains("adjust_market_buy_amount"),
        "stdout must not expose raw NT fee source content: {stdout}"
    );
    let json: serde_json::Value = serde_json::from_slice(
        &fs::read(&output_path).expect("CLOB V2 fee behavior source should write"),
    )
    .expect("CLOB V2 fee behavior source should be JSON");
    assert_eq!(json["schema_version"], 1);
    assert_eq!(
        json["record_kind"],
        "bolt_v3.pre_run_clob_v2_fee_behavior_source.v1"
    );
    assert_eq!(json["fee_behavior_verified"], true);
    assert_eq!(json["maker_zero_fee_verified"], true);
    assert_eq!(json["taker_fee_schedule_verified"], true);
    assert_eq!(json["market_buy_fee_adjustment_verified"], true);
    let price: f64 = json["price"]
        .as_str()
        .expect("price should be string")
        .parse()
        .expect("price should parse");
    assert!(price > 0.0 && price < 1.0);
    let fee_rate: f64 = json["fee_rate"]
        .as_str()
        .expect("fee_rate should be string")
        .parse()
        .expect("fee_rate should parse");
    assert!(fee_rate >= 0.0);
    for field in ["fee_behavior_source_sha256", "fee_assumptions_sha256"] {
        let value = json[field]
            .as_str()
            .unwrap_or_else(|| panic!("{field} should be a string: {json}"));
        assert!(
            value.len() == 64
                && value
                    .chars()
                    .all(|ch| ch.is_ascii_hexdigit() && !ch.is_ascii_uppercase()),
            "{field} should be lowercase sha256 hex: {value}"
        );
    }
}

#[test]
fn bolt_v3_cli_collects_egress_identity_source_from_configured_probe() {
    let temp = tempdir().expect("tempdir should create");
    let observed_identity = "198.51.100.17\n";
    let normalized_identity = observed_identity.trim();
    let approved_egress_identity_sha256 =
        hex::encode(Sha256::digest(normalized_identity.as_bytes()));
    let observed_identity_path = temp.path().join("observed-egress-identity.txt");
    fs::write(&observed_identity_path, observed_identity)
        .expect("observed egress identity probe source should write");

    let config_path = write_bolt_v3_fixture_root(|root| {
        format!(
            "{root}\n{}",
            live_canary_with_egress_identity_toml(
                observed_identity_path
                    .to_str()
                    .expect("probe path should be utf-8"),
                &approved_egress_identity_sha256,
            )
        )
    });
    let output_path = temp.path().join("egress-identity-source.json");

    let output = Command::new(env!("CARGO_BIN_EXE_bolt-v2"))
        .args([
            "operator-artifacts",
            "collect-pre-run-egress-identity-source",
            "--config",
            config_path.to_str().expect("fixture path should be utf-8"),
            "--strategy-instance-id",
            "configured_updown_main",
            "--output",
            output_path.to_str().expect("output path should be utf-8"),
        ])
        .output()
        .expect("bolt-v3 egress identity source collection should run");

    assert!(
        output.status.success(),
        "expected egress identity source collection to pass, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains(normalized_identity),
        "stdout must not expose raw egress identity: {stdout}"
    );
    let json: serde_json::Value = serde_json::from_slice(
        &fs::read(&output_path).expect("egress identity source should write"),
    )
    .expect("egress identity source should be JSON");
    assert_eq!(json["schema_version"], 1);
    assert_eq!(
        json["record_kind"],
        "bolt_v3.pre_run_egress_identity_source.v1"
    );
    assert_eq!(
        json["observed_egress_identity_sha256"],
        approved_egress_identity_sha256
    );
    assert_eq!(
        json["approved_egress_identity_sha256"],
        approved_egress_identity_sha256
    );
}

#[test]
fn bolt_v3_cli_collects_egress_identity_source_before_operator_evidence_patch() {
    let temp = tempdir().expect("tempdir should create");
    let observed_identity = "198.51.100.23\n";
    let normalized_identity = observed_identity.trim();
    let approved_egress_identity_sha256 =
        hex::encode(Sha256::digest(normalized_identity.as_bytes()));
    let observed_identity_path = temp.path().join("observed-egress-identity.txt");
    fs::write(&observed_identity_path, observed_identity)
        .expect("observed egress identity probe source should write");

    let config_path = write_bolt_v3_fixture_root(|root| {
        format!(
            "{root}\n{}",
            live_canary_with_egress_identity_toml(
                observed_identity_path
                    .to_str()
                    .expect("probe path should be utf-8"),
                &approved_egress_identity_sha256,
            )
        )
    });
    let output_path = temp.path().join("egress-identity-source.json");

    let output = Command::new(env!("CARGO_BIN_EXE_bolt-v2"))
        .args([
            "operator-artifacts",
            "collect-pre-run-egress-identity-source",
            "--config",
            config_path.to_str().expect("fixture path should be utf-8"),
            "--strategy-instance-id",
            "configured_updown_main",
            "--output",
            output_path.to_str().expect("output path should be utf-8"),
        ])
        .output()
        .expect("bolt-v3 egress identity source collection should run");

    assert!(
        output.status.success(),
        "expected egress identity source collection before operator evidence patch to pass, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout should be utf-8");
    assert!(!stdout.contains(normalized_identity), "{stdout}");
    assert!(
        output_path.exists(),
        "egress source artifact should be written"
    );
}

#[test]
fn bolt_v3_cli_collects_clob_v2_collateral_accounting_source_from_ssm_backed_balance_allowance() {
    let temp = tempdir().expect("tempdir should create");
    let (clob_url, clob_request_rx) =
        spawn_one_shot_clob_balance_allowance_server("1000000000", "999999999000000");
    let (ssm_url, ssm_paths_rx) = spawn_fake_ssm_server(BTreeMap::from([
        (
            "/bolt/polymarket/private-key",
            "0x1111111111111111111111111111111111111111111111111111111111111111",
        ),
        ("/bolt/polymarket/api-key", "poly-api-key"),
        ("/bolt/polymarket/api-secret", "YWJj"),
        ("/bolt/polymarket/api-passphrase", "poly-passphrase"),
        ("/bolt/testnet/chainlink/api-key", "chainlink-api-key"),
        ("/bolt/testnet/chainlink/api-secret", "chainlink-api-secret"),
    ]));
    let config_path = write_bolt_v3_fixture_root(|root| {
        format!(
            "{}\n{}",
            root.replace(
                "base_url_http = \"https://clob.polymarket.com\"",
                &format!("base_url_http = \"{clob_url}\""),
            ),
            live_canary_toml_without_operator_evidence()
        )
    });
    let fee_rate_source_path = write_cli_json_artifact(
        temp.path(),
        "fee-rate-source.json",
        serde_json::json!({
            "schema_version": 1,
            "record_kind": "bolt_v3.entry_decision_fee_rate_source.v1",
            "fee_bps_by_instrument_id": {
                "condition-token-up.POLYMARKET": 2.5,
                "condition-token-down.POLYMARKET": 3.5
            }
        }),
    );
    let fee_rate_source_sha256 = sha256_file_for_cli_test(&fee_rate_source_path);
    let output_path = temp
        .path()
        .join("clob-v2-collateral-accounting-source.json");

    let output = Command::new(env!("CARGO_BIN_EXE_bolt-v2"))
        .args([
            "operator-artifacts",
            "collect-pre-run-clob-v2-collateral-accounting-source",
            "--config",
            config_path.to_str().expect("config path should be utf-8"),
            "--strategy-instance-id",
            "configured_updown_main",
            "--fee-rate-source",
            fee_rate_source_path
                .to_str()
                .expect("fee source path should be utf-8"),
            "--fee-rate-source-sha256",
            &fee_rate_source_sha256,
            "--max-fee-rate-source-bytes",
            "100000",
            "--output",
            output_path.to_str().expect("output path should be utf-8"),
        ])
        .env("AWS_ENDPOINT_URL_SSM", &ssm_url)
        .env("AWS_ACCESS_KEY_ID", "fake-access-key")
        .env("AWS_SECRET_ACCESS_KEY", "fake-secret-key")
        .env("AWS_REGION", "eu-west-1")
        .env("AWS_MAX_ATTEMPTS", "1")
        .output()
        .expect("bolt-v3 CLOB V2 collateral source collection should run");

    assert!(
        output.status.success(),
        "expected CLOB V2 collateral source collection to pass, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    for forbidden in [
        "/bolt/polymarket/private-key",
        "poly-api-key",
        "poly-passphrase",
        "1000000000",
        "999999999000000",
    ] {
        assert!(
            !stdout.contains(forbidden),
            "stdout leaked {forbidden}: {stdout}"
        );
    }
    let request = clob_request_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("fake CLOB server should capture balance-allowance request");
    let request_lower = request.to_ascii_lowercase();
    assert!(request.starts_with("GET /balance-allowance?"), "{request}");
    assert!(request.contains("asset_type=COLLATERAL"), "{request}");
    assert!(request.contains("signature_type=1"), "{request}");
    for header in [
        "poly_address:",
        "poly_signature:",
        "poly_timestamp:",
        "poly_api_key:",
        "poly_passphrase:",
    ] {
        assert!(
            request_lower.contains(header),
            "missing auth header {header}: {request}"
        );
    }
    let paths = ssm_paths_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("fake SSM server should report requested paths");
    assert!(paths.contains(&"/bolt/polymarket/private-key".to_string()));
    assert!(paths.contains(&"/bolt/polymarket/api-key".to_string()));
    assert!(paths.contains(&"/bolt/polymarket/api-secret".to_string()));
    assert!(paths.contains(&"/bolt/polymarket/api-passphrase".to_string()));

    let json: serde_json::Value = serde_json::from_slice(
        &fs::read(&output_path).expect("CLOB V2 collateral source should write"),
    )
    .expect("CLOB V2 collateral source should be JSON");
    assert_eq!(json["schema_version"], 1);
    assert_eq!(
        json["record_kind"],
        "bolt_v3.pre_run_clob_v2_collateral_accounting_source.v1"
    );
    assert_eq!(json["collateral_accounting_verified"], true);
    assert_eq!(json["p_usd_balance"], "1000");
    assert_eq!(json["p_usd_allowance"], "999999999");
    assert_eq!(json["required_max_notional_plus_fees"], "10.0035");
    for field in [
        "collateral_accounting_source_sha256",
        "collateral_assumptions_sha256",
    ] {
        let value = json[field]
            .as_str()
            .unwrap_or_else(|| panic!("{field} should be a string: {json}"));
        assert!(
            value.len() == 64
                && value
                    .chars()
                    .all(|ch| ch.is_ascii_hexdigit() && !ch.is_ascii_uppercase()),
            "{field} should be lowercase sha256 hex: {value}"
        );
    }
}

#[test]
fn bolt_v3_cli_collects_clob_v2_collateral_accounting_source_from_on_chain_pusd_allowance() {
    let temp = tempdir().expect("tempdir should create");
    let (rpc_url, rpc_requests_rx) = spawn_eth_call_server_with_results([
        "0x000000000000000000000000000000000000000000000000000000003b9aca00",
        "0x0000000000000000000000000000000000000000000000000000000077359400",
        "0x0000000000000000000000000000000000000000000000000000000059682f00",
    ]);
    let token_address = "0x2222222222222222222222222222222222222222";
    let ctf_spender = format!("{CTF_EXCHANGE:#x}");
    let neg_risk_spender = format!("{NEG_RISK_CTF_EXCHANGE:#x}");
    let config_path = write_bolt_v3_fixture_root(|root| {
        let on_chain_collateral = format!(
            r#"
[clients.polymarket_main.execution.on_chain_collateral]
rpc_url = "{rpc_url}"
chain_id = 137
collateral_token_address = "{token_address}"
"#
        );
        format!(
            "{}\n{}",
            root.replace(
                "transport_backend = \"sockudo\"\n\n[clients.polymarket_main.secrets]",
                &format!(
                    "transport_backend = \"sockudo\"\n{on_chain_collateral}\n[clients.polymarket_main.secrets]"
                ),
            ),
            live_canary_toml_without_operator_evidence()
        )
    });
    let fee_rate_source_path = write_cli_json_artifact(
        temp.path(),
        "fee-rate-source.json",
        serde_json::json!({
            "schema_version": 1,
            "record_kind": "bolt_v3.entry_decision_fee_rate_source.v1",
            "fee_bps_by_instrument_id": {
                "condition-token-up.POLYMARKET": 2.5,
                "condition-token-down.POLYMARKET": 3.5
            }
        }),
    );
    let fee_rate_source_sha256 = sha256_file_for_cli_test(&fee_rate_source_path);
    let output_path = temp
        .path()
        .join("clob-v2-collateral-accounting-source.json");

    let output = Command::new(env!("CARGO_BIN_EXE_bolt-v2"))
        .args([
            "operator-artifacts",
            "collect-pre-run-clob-v2-collateral-accounting-source",
            "--config",
            config_path.to_str().expect("config path should be utf-8"),
            "--strategy-instance-id",
            "configured_updown_main",
            "--fee-rate-source",
            fee_rate_source_path
                .to_str()
                .expect("fee source path should be utf-8"),
            "--fee-rate-source-sha256",
            &fee_rate_source_sha256,
            "--max-fee-rate-source-bytes",
            "100000",
            "--output",
            output_path.to_str().expect("output path should be utf-8"),
        ])
        .output()
        .expect("bolt-v3 on-chain CLOB V2 collateral source collection should run");

    assert!(
        output.status.success(),
        "expected on-chain CLOB V2 collateral source collection to pass, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    for forbidden in [
        token_address,
        ctf_spender.as_str(),
        neg_risk_spender.as_str(),
        "1000000000",
        "2000000000",
        "1500000000",
    ] {
        assert!(
            !stdout.contains(forbidden),
            "stdout leaked {forbidden}: {stdout}"
        );
    }

    let requests = rpc_requests_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("fake RPC server should capture eth_call requests");
    assert_eq!(requests.len(), 3, "{requests:#?}");
    let joined_requests = requests.join("\n");
    assert!(
        joined_requests.matches("POST / HTTP/1.1").count() == 3,
        "{joined_requests}"
    );
    assert!(
        joined_requests.matches("\"method\":\"eth_call\"").count() == 3,
        "{joined_requests}"
    );
    assert!(
        joined_requests.matches("\"latest\"").count() == 3,
        "{joined_requests}"
    );
    assert!(joined_requests.contains("0x70a08231"), "{joined_requests}");
    assert_eq!(joined_requests.matches("0xdd62ed3e").count(), 2);
    assert!(
        joined_requests.contains(&token_address.to_ascii_lowercase()),
        "{joined_requests}"
    );
    assert!(
        joined_requests.contains(ctf_spender.trim_start_matches("0x")),
        "{joined_requests}"
    );
    assert!(
        joined_requests.contains(neg_risk_spender.trim_start_matches("0x")),
        "{joined_requests}"
    );

    let json: serde_json::Value = serde_json::from_slice(
        &fs::read(&output_path).expect("on-chain CLOB V2 collateral source should write"),
    )
    .expect("on-chain CLOB V2 collateral source should be JSON");
    assert_eq!(json["schema_version"], 1);
    assert_eq!(
        json["record_kind"],
        "bolt_v3.pre_run_clob_v2_collateral_accounting_source.v1"
    );
    assert_eq!(json["collateral_accounting_verified"], true);
    assert_eq!(json["p_usd_balance"], "1000");
    assert_eq!(json["p_usd_allowance"], "1500");
    assert_eq!(json["required_max_notional_plus_fees"], "10.0035");
}

#[test]
fn bolt_v3_cli_collects_clob_v2_collateral_accounting_source_accepts_on_chain_max_uint_allowance() {
    let temp = tempdir().expect("tempdir should create");
    let max_uint_word = "0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff";
    let (rpc_url, _rpc_requests_rx) = spawn_eth_call_server_with_results([
        "0x000000000000000000000000000000000000000000000000000000003b9aca00",
        max_uint_word,
        max_uint_word,
    ]);
    let config_path = write_bolt_v3_fixture_root(|root| {
        let on_chain_collateral = format!(
            r#"
[clients.polymarket_main.execution.on_chain_collateral]
rpc_url = "{rpc_url}"
chain_id = 137
collateral_token_address = "0x2222222222222222222222222222222222222222"
"#
        );
        format!(
            "{}\n{}",
            root.replace(
                "transport_backend = \"sockudo\"\n\n[clients.polymarket_main.secrets]",
                &format!(
                    "transport_backend = \"sockudo\"\n{on_chain_collateral}\n[clients.polymarket_main.secrets]"
                ),
            ),
            live_canary_toml_without_operator_evidence()
        )
    });
    let fee_rate_source_path = write_cli_json_artifact(
        temp.path(),
        "fee-rate-source.json",
        serde_json::json!({
            "schema_version": 1,
            "record_kind": "bolt_v3.entry_decision_fee_rate_source.v1",
            "fee_bps_by_instrument_id": {
                "condition-token-up.POLYMARKET": 2.5,
                "condition-token-down.POLYMARKET": 3.5
            }
        }),
    );
    let fee_rate_source_sha256 = sha256_file_for_cli_test(&fee_rate_source_path);
    let output_path = temp
        .path()
        .join("clob-v2-collateral-accounting-source.json");

    let output = Command::new(env!("CARGO_BIN_EXE_bolt-v2"))
        .args([
            "operator-artifacts",
            "collect-pre-run-clob-v2-collateral-accounting-source",
            "--config",
            config_path.to_str().expect("config path should be utf-8"),
            "--strategy-instance-id",
            "configured_updown_main",
            "--fee-rate-source",
            fee_rate_source_path
                .to_str()
                .expect("fee source path should be utf-8"),
            "--fee-rate-source-sha256",
            &fee_rate_source_sha256,
            "--max-fee-rate-source-bytes",
            "100000",
            "--output",
            output_path.to_str().expect("output path should be utf-8"),
        ])
        .output()
        .expect("bolt-v3 on-chain max allowance source collection should run");

    assert!(
        output.status.success(),
        "expected max-uint on-chain CLOB V2 collateral source collection to pass, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let json: serde_json::Value = serde_json::from_slice(
        &fs::read(&output_path).expect("on-chain CLOB V2 collateral source should write"),
    )
    .expect("on-chain CLOB V2 collateral source should be JSON");
    assert_eq!(json["p_usd_balance"], "1000");
    assert_eq!(
        json["p_usd_allowance"],
        "115792089237316195423570985008687907853269984665640564039457584007913129.639935"
    );
}

#[test]
fn bolt_v3_cli_exposes_clob_v2_collateral_accounting_source_from_configured_balance_allowance() {
    let output = Command::new(env!("CARGO_BIN_EXE_bolt-v2"))
        .args([
            "operator-artifacts",
            "collect-pre-run-clob-v2-collateral-accounting-source",
            "--help",
        ])
        .output()
        .expect("bolt-v3 CLOB V2 collateral accounting source help should run");

    assert!(
        output.status.success(),
        "expected CLOB V2 collateral accounting source help to pass, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("--config"), "{stdout}");
    assert!(stdout.contains("--strategy-instance-id"), "{stdout}");
    assert!(stdout.contains("--fee-rate-source"), "{stdout}");
    assert!(stdout.contains("--fee-rate-source-sha256"), "{stdout}");
    assert!(stdout.contains("--max-fee-rate-source-bytes"), "{stdout}");
    assert!(stdout.contains("--output"), "{stdout}");
    for forbidden in [
        "--p-usd-balance",
        "--p-usd-allowance",
        "--required-max-notional-plus-fees",
    ] {
        assert!(
            !stdout.contains(forbidden),
            "CLOB V2 collateral materializer must not accept caller-supplied runtime value {forbidden}: {stdout}"
        );
    }
}

#[test]
fn bolt_v3_cli_collects_venue_account_state_source_from_configured_account_queries() {
    let temp = tempdir().expect("tempdir should create");
    let (clob_url, clob_request_rx) = spawn_one_shot_clob_open_orders_server();
    let (data_api_url, data_api_request_rx) = spawn_one_shot_data_api_positions_server();
    let (ssm_url, ssm_paths_rx) = spawn_fake_ssm_server(BTreeMap::from([
        (
            "/bolt/polymarket/private-key",
            "0x1111111111111111111111111111111111111111111111111111111111111111",
        ),
        ("/bolt/polymarket/api-key", "poly-api-key"),
        ("/bolt/polymarket/api-secret", "YWJj"),
        ("/bolt/polymarket/api-passphrase", "poly-passphrase"),
        ("/bolt/testnet/chainlink/api-key", "chainlink-api-key"),
        ("/bolt/testnet/chainlink/api-secret", "chainlink-api-secret"),
    ]));
    let config_path = write_bolt_v3_fixture_root(|root| {
        format!(
            "{}\n{}",
            root.replace(
                "base_url_http = \"https://clob.polymarket.com\"",
                &format!("base_url_http = \"{clob_url}\""),
            )
            .replace(
                "base_url_data_api = \"https://data-api.polymarket.com\"",
                &format!("base_url_data_api = \"{data_api_url}\""),
            ),
            live_canary_toml_without_operator_evidence()
        )
    });
    let output_path = temp.path().join("venue-account-state-source.json");

    let output = Command::new(env!("CARGO_BIN_EXE_bolt-v2"))
        .args([
            "operator-artifacts",
            "collect-pre-run-venue-account-state-source",
            "--config",
            config_path.to_str().expect("config path should be utf-8"),
            "--strategy-instance-id",
            "configured_updown_main",
            "--output",
            output_path.to_str().expect("output path should be utf-8"),
        ])
        .env("AWS_ENDPOINT_URL_SSM", &ssm_url)
        .env("AWS_ACCESS_KEY_ID", "fake-access-key")
        .env("AWS_SECRET_ACCESS_KEY", "fake-secret-key")
        .env("AWS_REGION", "eu-west-1")
        .env("AWS_MAX_ATTEMPTS", "1")
        .output()
        .expect("bolt-v3 venue account source collection should run");

    assert!(
        output.status.success(),
        "expected venue account source collection to pass, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    for forbidden in [
        "/bolt/polymarket/private-key",
        "poly-api-key",
        "poly-passphrase",
        "0x1111111111111111111111111111111111111111",
    ] {
        assert!(
            !stdout.contains(forbidden),
            "stdout leaked {forbidden}: {stdout}"
        );
    }
    let clob_request = clob_request_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("fake CLOB server should capture open-orders request");
    let clob_request_lower = clob_request.to_ascii_lowercase();
    assert!(
        clob_request.starts_with("GET /data/orders?"),
        "{clob_request}"
    );
    for header in [
        "poly_address:",
        "poly_signature:",
        "poly_timestamp:",
        "poly_api_key:",
        "poly_passphrase:",
    ] {
        assert!(
            clob_request_lower.contains(header),
            "missing auth header {header}: {clob_request}"
        );
    }
    let data_api_request = data_api_request_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("fake Data API server should capture positions request");
    assert!(
        data_api_request.starts_with("GET /positions?"),
        "{data_api_request}"
    );
    assert!(
        data_api_request.contains("user=0x1111111111111111111111111111111111111111"),
        "{data_api_request}"
    );
    let paths = ssm_paths_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("fake SSM server should report requested paths");
    assert!(paths.contains(&"/bolt/polymarket/private-key".to_string()));
    assert!(paths.contains(&"/bolt/polymarket/api-key".to_string()));
    assert!(paths.contains(&"/bolt/polymarket/api-secret".to_string()));
    assert!(paths.contains(&"/bolt/polymarket/api-passphrase".to_string()));

    let json: serde_json::Value = serde_json::from_slice(
        &fs::read(&output_path).expect("venue account state source should write"),
    )
    .expect("venue account state source should be JSON");
    assert_eq!(json["schema_version"], 1);
    assert_eq!(
        json["record_kind"],
        "bolt_v3.pre_run_venue_account_state_source.v1"
    );
    assert_eq!(json["execution_client_id"], "polymarket_main");
    assert_eq!(json["configured_target_id"], "configured_updown_target");
    assert_eq!(json["open_order_count"], 0);
    assert_eq!(json["open_position_count"], 0);
    let snapshot_sha = json["account_state_snapshot_sha256"]
        .as_str()
        .unwrap_or_else(|| panic!("snapshot sha should be a string: {json}"));
    assert!(
        snapshot_sha.len() == 64
            && snapshot_sha
                .chars()
                .all(|ch| ch.is_ascii_hexdigit() && !ch.is_ascii_uppercase()),
        "snapshot sha should be lowercase sha256 hex: {snapshot_sha}"
    );
}

#[test]
fn bolt_v3_cli_collects_venue_account_state_source_ignores_zero_and_dust_positions() {
    let temp = tempdir().expect("tempdir should create");
    let output_path = temp.path().join("venue-account-state-source.json");
    let output = run_venue_account_state_source_fixture(
        format!(
            r#"[
                {{
                    "asset": "zero-token",
                    "conditionId": "0xzero-condition",
                    "size": 0.0,
                    "avgPrice": null
                }},
                {{
                    "asset": "dust-token",
                    "conditionId": "0xdust-condition",
                    "size": {},
                    "avgPrice": null
                }}
            ]"#,
            DUST_POSITION_THRESHOLD / 2.0
        ),
        &output_path,
    );

    assert!(
        output.status.success(),
        "expected zero and dust positions to be ignored, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(
        &fs::read(&output_path).expect("venue account state source should write"),
    )
    .expect("venue account state source should be JSON");
    assert_eq!(json["open_order_count"], 0);
    assert_eq!(json["open_position_count"], 0);
}

#[test]
fn bolt_v3_cli_collects_venue_account_state_source_rejects_active_position() {
    let temp = tempdir().expect("tempdir should create");
    let output_path = temp.path().join("venue-account-state-source.json");
    let output = run_venue_account_state_source_fixture(
        format!(
            r#"[
                {{
                    "asset": "active-token",
                    "conditionId": "0xactive-condition",
                    "size": {},
                    "avgPrice": null
                }}
            ]"#,
            DUST_POSITION_THRESHOLD
        ),
        &output_path,
    );

    assert!(
        !output.status.success(),
        "expected active position to block venue account source collection"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("preexisting_position_absent"),
        "expected active position failure to identify preexisting_position_absent, got: {stderr}"
    );
}

#[test]
fn bolt_v3_cli_collects_venue_account_state_source_confirms_transient_open_order_before_blocking() {
    let temp = tempdir().expect("tempdir should create");
    let output_path = temp.path().join("venue-account-state-source.json");
    // The fixture's confirmation loop sets max_retries = 3
    // (tests/fixtures/bolt_v3/root.toml:162), so the fail-closed snapshot
    // requires 3 consecutive clear reads after the transient blocking read:
    // 1 blocking + 3 clears = 4 open-orders bodies.
    let output = run_venue_account_state_source_fixture_with_responses(
        [
            open_orders_body_with_one_live_order(),
            empty_open_orders_body(),
            empty_open_orders_body(),
            empty_open_orders_body(),
        ],
        ["[]".into()],
        &output_path,
    );

    assert!(
        output.status.success(),
        "expected transient open-order snapshot to be confirmed before blocking, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(
        &fs::read(&output_path).expect("venue account state source should write"),
    )
    .expect("venue account state source should be JSON");
    assert_eq!(json["open_order_count"], 0);
    assert_eq!(json["open_position_count"], 0);
}

#[test]
fn bolt_v3_cli_collects_venue_account_state_source_keeps_persistent_open_order_blocking() {
    let temp = tempdir().expect("tempdir should create");
    let output_path = temp.path().join("venue-account-state-source.json");
    let output = run_venue_account_state_source_fixture_with_responses(
        [
            open_orders_body_with_one_live_order(),
            open_orders_body_with_one_live_order(),
            open_orders_body_with_one_live_order(),
            open_orders_body_with_one_live_order(),
        ],
        ["[]".into()],
        &output_path,
    );

    assert!(
        !output.status.success(),
        "expected persistent open order to remain blocking"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("conflicting_open_orders_absent"),
        "expected open-order failure to identify conflicting_open_orders_absent, got: {stderr}"
    );
    assert!(
        !output_path.exists(),
        "blocking venue account source should not write an output artifact"
    );
}

#[test]
fn bolt_v3_cli_collects_venue_account_state_source_confirms_transient_active_position_before_blocking()
 {
    let temp = tempdir().expect("tempdir should create");
    let output_path = temp.path().join("venue-account-state-source.json");
    let active_positions = format!(
        r#"[
            {{
                "asset": "stale-active-token",
                "conditionId": "0xstale-active-condition",
                "size": {},
                "avgPrice": null
            }}
        ]"#,
        DUST_POSITION_THRESHOLD
    );
    // The fixture's confirmation loop sets max_retries = 3
    // (tests/fixtures/bolt_v3/root.toml:162), so the fail-closed snapshot
    // requires 3 consecutive clear reads after the transient blocking read:
    // 1 blocking + 3 clears = 4 data-api position bodies.
    let output = run_venue_account_state_source_fixture_with_data_api_bodies(
        [active_positions, "[]".into(), "[]".into(), "[]".into()],
        &output_path,
    );

    assert!(
        output.status.success(),
        "expected transient active position snapshot to be confirmed before blocking, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(
        &fs::read(&output_path).expect("venue account state source should write"),
    )
    .expect("venue account state source should be JSON");
    assert_eq!(json["open_order_count"], 0);
    assert_eq!(json["open_position_count"], 0);
}

fn run_venue_account_state_source_fixture(
    data_api_positions_body: impl Into<String>,
    output_path: &Path,
) -> Output {
    run_venue_account_state_source_fixture_with_data_api_bodies(
        [data_api_positions_body.into()],
        output_path,
    )
}

fn run_venue_account_state_source_fixture_with_data_api_bodies(
    data_api_positions_bodies: impl IntoIterator<Item = String>,
    output_path: &Path,
) -> Output {
    run_venue_account_state_source_fixture_with_responses(
        [empty_open_orders_body()],
        data_api_positions_bodies,
        output_path,
    )
}

fn run_venue_account_state_source_fixture_with_responses(
    clob_open_orders_bodies: impl IntoIterator<Item = String>,
    data_api_positions_bodies: impl IntoIterator<Item = String>,
    output_path: &Path,
) -> Output {
    let (clob_url, _clob_request_rx) =
        spawn_clob_open_orders_server_with_bodies(clob_open_orders_bodies);
    let (data_api_url, _data_api_request_rx) =
        spawn_data_api_positions_server_with_bodies(data_api_positions_bodies);
    let (ssm_url, _ssm_paths_rx) = spawn_fake_ssm_server(BTreeMap::from([
        (
            "/bolt/polymarket/private-key",
            "0x1111111111111111111111111111111111111111111111111111111111111111",
        ),
        ("/bolt/polymarket/api-key", "poly-api-key"),
        ("/bolt/polymarket/api-secret", "YWJj"),
        ("/bolt/polymarket/api-passphrase", "poly-passphrase"),
        ("/bolt/testnet/chainlink/api-key", "chainlink-api-key"),
        ("/bolt/testnet/chainlink/api-secret", "chainlink-api-secret"),
    ]));
    let config_path = write_bolt_v3_fixture_root(|root| {
        format!(
            "{}\n{}",
            root.replace(
                "base_url_http = \"https://clob.polymarket.com\"",
                &format!("base_url_http = \"{clob_url}\""),
            )
            .replace(
                "base_url_data_api = \"https://data-api.polymarket.com\"",
                &format!("base_url_data_api = \"{data_api_url}\""),
            ),
            live_canary_toml_without_operator_evidence()
        )
    });

    Command::new(env!("CARGO_BIN_EXE_bolt-v2"))
        .args([
            "operator-artifacts",
            "collect-pre-run-venue-account-state-source",
            "--config",
            config_path.to_str().expect("config path should be utf-8"),
            "--strategy-instance-id",
            "configured_updown_main",
            "--output",
            output_path.to_str().expect("output path should be utf-8"),
        ])
        .env("AWS_ENDPOINT_URL_SSM", &ssm_url)
        .env("AWS_ACCESS_KEY_ID", "fake-access-key")
        .env("AWS_SECRET_ACCESS_KEY", "fake-secret-key")
        .env("AWS_REGION", "eu-west-1")
        .env("AWS_MAX_ATTEMPTS", "1")
        .output()
        .expect("bolt-v3 venue account source collection should run")
}

#[test]
fn bolt_v3_cli_exposes_venue_account_state_source_from_configured_account_queries() {
    let output = Command::new(env!("CARGO_BIN_EXE_bolt-v2"))
        .args([
            "operator-artifacts",
            "collect-pre-run-venue-account-state-source",
            "--help",
        ])
        .output()
        .expect("bolt-v3 venue account source help should run");

    assert!(
        output.status.success(),
        "expected venue account source help to pass, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("--config"), "{stdout}");
    assert!(stdout.contains("--strategy-instance-id"), "{stdout}");
    assert!(stdout.contains("--output"), "{stdout}");
    for forbidden in [
        "--open-order-count",
        "--open-position-count",
        "--account-state-snapshot-sha256",
    ] {
        assert!(
            !stdout.contains(forbidden),
            "help exposes caller-supplied source field {forbidden}: {stdout}"
        );
    }
}

#[test]
fn bolt_v3_cli_collects_funding_margin_source_from_ssm_backed_balance_allowance() {
    let temp = tempdir().expect("tempdir should create");
    let (clob_url, clob_request_rx) =
        spawn_one_shot_clob_balance_allowance_server("1000000000", "999999999000000");
    let (ssm_url, ssm_paths_rx) = spawn_fake_ssm_server(BTreeMap::from([
        (
            "/bolt/polymarket/private-key",
            "0x1111111111111111111111111111111111111111111111111111111111111111",
        ),
        ("/bolt/polymarket/api-key", "poly-api-key"),
        ("/bolt/polymarket/api-secret", "YWJj"),
        ("/bolt/polymarket/api-passphrase", "poly-passphrase"),
        ("/bolt/testnet/chainlink/api-key", "chainlink-api-key"),
        ("/bolt/testnet/chainlink/api-secret", "chainlink-api-secret"),
    ]));
    let config_path = write_bolt_v3_fixture_root(|root| {
        format!(
            "{}\n{}",
            root.replace(
                "base_url_http = \"https://clob.polymarket.com\"",
                &format!("base_url_http = \"{clob_url}\""),
            ),
            live_canary_toml_without_operator_evidence()
        )
    });
    let fee_rate_source_path = write_cli_json_artifact(
        temp.path(),
        "fee-rate-source.json",
        serde_json::json!({
            "schema_version": 1,
            "record_kind": "bolt_v3.entry_decision_fee_rate_source.v1",
            "fee_bps_by_instrument_id": {
                "condition-token-up.POLYMARKET": 2.5,
                "condition-token-down.POLYMARKET": 3.5
            }
        }),
    );
    let fee_rate_source_sha256 = sha256_file_for_cli_test(&fee_rate_source_path);
    let output_path = temp.path().join("funding-margin-source.json");

    let output = Command::new(env!("CARGO_BIN_EXE_bolt-v2"))
        .args([
            "operator-artifacts",
            "collect-pre-run-funding-margin-source",
            "--config",
            config_path.to_str().expect("config path should be utf-8"),
            "--strategy-instance-id",
            "configured_updown_main",
            "--fee-rate-source",
            fee_rate_source_path
                .to_str()
                .expect("fee source path should be utf-8"),
            "--fee-rate-source-sha256",
            &fee_rate_source_sha256,
            "--max-fee-rate-source-bytes",
            "100000",
            "--output",
            output_path.to_str().expect("output path should be utf-8"),
        ])
        .env("AWS_ENDPOINT_URL_SSM", &ssm_url)
        .env("AWS_ACCESS_KEY_ID", "fake-access-key")
        .env("AWS_SECRET_ACCESS_KEY", "fake-secret-key")
        .env("AWS_REGION", "eu-west-1")
        .env("AWS_MAX_ATTEMPTS", "1")
        .output()
        .expect("bolt-v3 funding margin source collection should run");

    assert!(
        output.status.success(),
        "expected funding margin source collection to pass, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    for forbidden in [
        "/bolt/polymarket/private-key",
        "poly-api-key",
        "poly-passphrase",
        "1000000000",
        "999999999000000",
    ] {
        assert!(
            !stdout.contains(forbidden),
            "stdout leaked {forbidden}: {stdout}"
        );
    }
    let request = clob_request_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("fake CLOB server should capture balance-allowance request");
    assert!(request.starts_with("GET /balance-allowance?"), "{request}");
    assert!(request.contains("asset_type=COLLATERAL"), "{request}");
    let paths = ssm_paths_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("fake SSM server should report requested paths");
    assert!(paths.contains(&"/bolt/polymarket/private-key".to_string()));
    assert!(paths.contains(&"/bolt/polymarket/api-key".to_string()));
    assert!(paths.contains(&"/bolt/polymarket/api-secret".to_string()));
    assert!(paths.contains(&"/bolt/polymarket/api-passphrase".to_string()));

    let json: serde_json::Value =
        serde_json::from_slice(&fs::read(&output_path).expect("funding source should write"))
            .expect("funding source should be JSON");
    assert_eq!(json["schema_version"], 1);
    assert_eq!(
        json["record_kind"],
        "bolt_v3.pre_run_funding_margin_source.v1"
    );
    assert_eq!(json["available_collateral"], "1000");
    assert_eq!(json["required_max_notional_plus_fees"], "10.0035");
    let snapshot_sha = json["margin_snapshot_sha256"]
        .as_str()
        .unwrap_or_else(|| panic!("margin snapshot sha should be a string: {json}"));
    assert!(
        snapshot_sha.len() == 64
            && snapshot_sha
                .chars()
                .all(|ch| ch.is_ascii_hexdigit() && !ch.is_ascii_uppercase()),
        "margin snapshot sha should be lowercase sha256 hex: {snapshot_sha}"
    );
}

#[test]
fn bolt_v3_cli_collects_clob_v2_collateral_accounting_source_confirms_transient_low_balance_allowance_before_blocking()
 {
    let temp = tempdir().expect("tempdir should create");
    // The fixture's confirmation loop sets max_retries = 3
    // (tests/fixtures/bolt_v3/root.toml:162), so the fail-closed snapshot
    // requires 3 consecutive clear reads after the transient blocking read:
    // 1 low + 3 sufficient balance-allowance bodies.
    let (clob_url, _clob_request_rx) = spawn_clob_balance_allowance_server_with_bodies([
        balance_allowance_body("0", "0"),
        balance_allowance_body("1000000000", "999999999000000"),
        balance_allowance_body("1000000000", "999999999000000"),
        balance_allowance_body("1000000000", "999999999000000"),
    ]);
    let output_path = temp
        .path()
        .join("clob-v2-collateral-accounting-source.json");

    let output = run_clob_v2_collateral_accounting_source_fixture(&clob_url, &temp, &output_path);

    assert!(
        output.status.success(),
        "expected transient low balance/allowance to be confirmed before blocking, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(
        &fs::read(&output_path).expect("CLOB V2 collateral source should write"),
    )
    .expect("CLOB V2 collateral source should be JSON");
    assert_eq!(json["collateral_accounting_verified"], true);
    assert_eq!(json["p_usd_balance"], "1000");
    assert_eq!(json["p_usd_allowance"], "999999999");
}

#[test]
fn bolt_v3_cli_syncs_clob_v2_balance_allowance_cache_from_configured_account() {
    let (clob_url, clob_request_rx) = spawn_one_shot_clob_balance_allowance_update_server();
    let (ssm_url, ssm_paths_rx) = spawn_fake_ssm_server(BTreeMap::from([
        (
            "/bolt/polymarket/private-key",
            "0x1111111111111111111111111111111111111111111111111111111111111111",
        ),
        ("/bolt/polymarket/api-key", "poly-api-key"),
        ("/bolt/polymarket/api-secret", "YWJj"),
        ("/bolt/polymarket/api-passphrase", "poly-passphrase"),
        ("/bolt/testnet/chainlink/api-key", "chainlink-api-key"),
        ("/bolt/testnet/chainlink/api-secret", "chainlink-api-secret"),
    ]));
    let config_path = write_bolt_v3_fixture_root(|root| {
        format!(
            "{}\n{}",
            root.replace(
                "base_url_http = \"https://clob.polymarket.com\"",
                &format!("base_url_http = \"{clob_url}\""),
            ),
            live_canary_toml_without_operator_evidence()
        )
    });

    let output = Command::new(env!("CARGO_BIN_EXE_bolt-v2"))
        .args([
            "operator-artifacts",
            "sync-clob-v2-balance-allowance-cache",
            "--config",
            config_path.to_str().expect("config path should be utf-8"),
            "--strategy-instance-id",
            "configured_updown_main",
            "--acknowledge-clob-cache-mutation",
        ])
        .env("AWS_ENDPOINT_URL_SSM", &ssm_url)
        .env("AWS_ACCESS_KEY_ID", "fake-access-key")
        .env("AWS_SECRET_ACCESS_KEY", "fake-secret-key")
        .env("AWS_REGION", "eu-west-1")
        .env("AWS_MAX_ATTEMPTS", "1")
        .output()
        .expect("bolt-v3 CLOB V2 cache sync should run");

    assert!(
        output.status.success(),
        "expected CLOB V2 cache sync to pass, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    for forbidden in [
        "/bolt/polymarket/private-key",
        "poly-api-key",
        "poly-passphrase",
        "0x1111111111111111111111111111111111111111",
    ] {
        assert!(
            !stdout.contains(forbidden),
            "stdout leaked {forbidden}: {stdout}"
        );
    }
    assert!(
        stdout.contains("\"clob_v2_balance_allowance_cache_sync_completed\": true"),
        "{stdout}"
    );

    let request = clob_request_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("fake CLOB server should capture balance-allowance update request");
    let request_lower = request.to_ascii_lowercase();
    assert!(
        request.starts_with("GET /balance-allowance/update?"),
        "{request}"
    );
    assert!(request.contains("asset_type=COLLATERAL"), "{request}");
    assert!(request.contains("signature_type=1"), "{request}");
    for header in [
        "poly_address:",
        "poly_signature:",
        "poly_timestamp:",
        "poly_api_key:",
        "poly_passphrase:",
    ] {
        assert!(
            request_lower.contains(header),
            "missing auth header {header}: {request}"
        );
    }
    let paths = ssm_paths_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("fake SSM server should report requested paths");
    assert!(paths.contains(&"/bolt/polymarket/private-key".to_string()));
    assert!(paths.contains(&"/bolt/polymarket/api-key".to_string()));
    assert!(paths.contains(&"/bolt/polymarket/api-secret".to_string()));
    assert!(paths.contains(&"/bolt/polymarket/api-passphrase".to_string()));
}

#[test]
fn bolt_v3_cli_collects_clob_v2_collateral_accounting_source_keeps_persistent_low_balance_blocking()
{
    let temp = tempdir().expect("tempdir should create");
    let (clob_url, _clob_request_rx) = spawn_clob_balance_allowance_server_with_bodies([
        balance_allowance_body("0", "0"),
        balance_allowance_body("0", "0"),
        balance_allowance_body("0", "0"),
        balance_allowance_body("0", "0"),
    ]);
    let output_path = temp
        .path()
        .join("clob-v2-collateral-accounting-source.json");

    let output = run_clob_v2_collateral_accounting_source_fixture(&clob_url, &temp, &output_path);

    assert!(
        !output.status.success(),
        "expected persistent low balance/allowance to remain blocking"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("collateral_accounting_verified"),
        "expected collateral failure to identify collateral_accounting_verified, got: {stderr}"
    );
    assert!(
        !output_path.exists(),
        "blocking CLOB V2 collateral source should not write an output artifact"
    );
}

#[test]
fn bolt_v3_cli_collects_clob_v2_collateral_accounting_source_keeps_blocking_when_confirmation_fetch_fails()
 {
    let temp = tempdir().expect("tempdir should create");
    let (clob_url, _clob_request_rx) = spawn_clob_balance_allowance_server_with_statuses([
        (200, balance_allowance_body("0", "0")),
        (500, r#"{"error":"temporary"}"#.to_string()),
    ]);
    let output_path = temp
        .path()
        .join("clob-v2-collateral-accounting-source.json");

    let output = run_clob_v2_collateral_accounting_source_fixture(&clob_url, &temp, &output_path);

    assert!(
        !output.status.success(),
        "expected confirmation fetch failure to keep low balance/allowance blocking"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("collateral_accounting_verified"),
        "expected collateral failure to identify collateral_accounting_verified, got: {stderr}"
    );
    assert!(
        !output_path.exists(),
        "confirmation fetch failure should not write an output artifact"
    );
}

#[test]
fn bolt_v3_cli_collects_funding_margin_source_confirms_transient_low_balance_allowance_before_blocking()
 {
    let temp = tempdir().expect("tempdir should create");
    // The fixture's confirmation loop sets max_retries = 3
    // (tests/fixtures/bolt_v3/root.toml:162), so the fail-closed snapshot
    // requires 3 consecutive clear reads after the transient blocking read:
    // 1 low + 3 sufficient balance-allowance bodies.
    let (clob_url, _clob_request_rx) = spawn_clob_balance_allowance_server_with_bodies([
        balance_allowance_body("0", "0"),
        balance_allowance_body("1000000000", "999999999000000"),
        balance_allowance_body("1000000000", "999999999000000"),
        balance_allowance_body("1000000000", "999999999000000"),
    ]);
    let output_path = temp.path().join("funding-margin-source.json");

    let output = run_funding_margin_source_fixture(&clob_url, &temp, &output_path);

    assert!(
        output.status.success(),
        "expected transient low balance/allowance to be confirmed before blocking, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value =
        serde_json::from_slice(&fs::read(&output_path).expect("funding source should write"))
            .expect("funding source should be JSON");
    assert_eq!(json["available_collateral"], "1000");
    assert_eq!(json["required_max_notional_plus_fees"], "10.0035");
}

#[test]
fn bolt_v3_cli_collects_funding_margin_source_keeps_persistent_low_balance_blocking() {
    let temp = tempdir().expect("tempdir should create");
    let (clob_url, _clob_request_rx) = spawn_clob_balance_allowance_server_with_bodies([
        balance_allowance_body("0", "0"),
        balance_allowance_body("0", "0"),
        balance_allowance_body("0", "0"),
        balance_allowance_body("0", "0"),
    ]);
    let output_path = temp.path().join("funding-margin-source.json");

    let output = run_funding_margin_source_fixture(&clob_url, &temp, &output_path);

    assert!(
        !output.status.success(),
        "expected persistent low balance/allowance to remain blocking"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("funding_margin_covers_max_notional_plus_fees"),
        "expected funding failure to identify funding_margin_covers_max_notional_plus_fees, got: {stderr}"
    );
    assert!(
        !output_path.exists(),
        "blocking funding margin source should not write an output artifact"
    );
}

#[test]
fn bolt_v3_cli_exposes_funding_margin_source_from_configured_balance_allowance() {
    let output = Command::new(env!("CARGO_BIN_EXE_bolt-v2"))
        .args([
            "operator-artifacts",
            "collect-pre-run-funding-margin-source",
            "--help",
        ])
        .output()
        .expect("bolt-v3 funding margin source help should run");

    assert!(
        output.status.success(),
        "expected funding margin source help to pass, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("--config"), "{stdout}");
    assert!(stdout.contains("--strategy-instance-id"), "{stdout}");
    assert!(stdout.contains("--fee-rate-source"), "{stdout}");
    assert!(stdout.contains("--fee-rate-source-sha256"), "{stdout}");
    assert!(stdout.contains("--max-fee-rate-source-bytes"), "{stdout}");
    assert!(stdout.contains("--output"), "{stdout}");
    for forbidden in [
        "--available-collateral",
        "--required-max-notional-plus-fees",
        "--margin-snapshot-sha256",
    ] {
        assert!(
            !stdout.contains(forbidden),
            "help exposes caller-supplied source field {forbidden}: {stdout}"
        );
    }
}

fn run_clob_v2_collateral_accounting_source_fixture(
    clob_url: &str,
    temp: &tempfile::TempDir,
    output_path: &Path,
) -> Output {
    let (ssm_url, _ssm_paths_rx) = spawn_fake_ssm_server(BTreeMap::from([
        (
            "/bolt/polymarket/private-key",
            "0x1111111111111111111111111111111111111111111111111111111111111111",
        ),
        ("/bolt/polymarket/api-key", "poly-api-key"),
        ("/bolt/polymarket/api-secret", "YWJj"),
        ("/bolt/polymarket/api-passphrase", "poly-passphrase"),
        ("/bolt/testnet/chainlink/api-key", "chainlink-api-key"),
        ("/bolt/testnet/chainlink/api-secret", "chainlink-api-secret"),
    ]));
    let config_path = write_bolt_v3_fixture_root(|root| {
        format!(
            "{}\n{}",
            root.replace(
                "base_url_http = \"https://clob.polymarket.com\"",
                &format!("base_url_http = \"{clob_url}\""),
            ),
            live_canary_toml_without_operator_evidence()
        )
    });
    let fee_rate_source_path = write_cli_json_artifact(
        temp.path(),
        "fee-rate-source.json",
        serde_json::json!({
            "schema_version": 1,
            "record_kind": "bolt_v3.entry_decision_fee_rate_source.v1",
            "fee_bps_by_instrument_id": {
                "condition-token-up.POLYMARKET": 2.5,
                "condition-token-down.POLYMARKET": 3.5
            }
        }),
    );
    let fee_rate_source_sha256 = sha256_file_for_cli_test(&fee_rate_source_path);

    Command::new(env!("CARGO_BIN_EXE_bolt-v2"))
        .args([
            "operator-artifacts",
            "collect-pre-run-clob-v2-collateral-accounting-source",
            "--config",
            config_path.to_str().expect("config path should be utf-8"),
            "--strategy-instance-id",
            "configured_updown_main",
            "--fee-rate-source",
            fee_rate_source_path
                .to_str()
                .expect("fee source path should be utf-8"),
            "--fee-rate-source-sha256",
            &fee_rate_source_sha256,
            "--max-fee-rate-source-bytes",
            "100000",
            "--output",
            output_path.to_str().expect("output path should be utf-8"),
        ])
        .env("AWS_ENDPOINT_URL_SSM", &ssm_url)
        .env("AWS_ACCESS_KEY_ID", "fake-access-key")
        .env("AWS_SECRET_ACCESS_KEY", "fake-secret-key")
        .env("AWS_REGION", "eu-west-1")
        .env("AWS_MAX_ATTEMPTS", "1")
        .output()
        .expect("bolt-v3 CLOB V2 collateral source collection should run")
}

fn run_funding_margin_source_fixture(
    clob_url: &str,
    temp: &tempfile::TempDir,
    output_path: &Path,
) -> Output {
    let (ssm_url, _ssm_paths_rx) = spawn_fake_ssm_server(BTreeMap::from([
        (
            "/bolt/polymarket/private-key",
            "0x1111111111111111111111111111111111111111111111111111111111111111",
        ),
        ("/bolt/polymarket/api-key", "poly-api-key"),
        ("/bolt/polymarket/api-secret", "YWJj"),
        ("/bolt/polymarket/api-passphrase", "poly-passphrase"),
        ("/bolt/testnet/chainlink/api-key", "chainlink-api-key"),
        ("/bolt/testnet/chainlink/api-secret", "chainlink-api-secret"),
    ]));
    let config_path = write_bolt_v3_fixture_root(|root| {
        format!(
            "{}\n{}",
            root.replace(
                "base_url_http = \"https://clob.polymarket.com\"",
                &format!("base_url_http = \"{clob_url}\""),
            ),
            live_canary_toml_without_operator_evidence()
        )
    });
    let fee_rate_source_path = write_cli_json_artifact(
        temp.path(),
        "fee-rate-source.json",
        serde_json::json!({
            "schema_version": 1,
            "record_kind": "bolt_v3.entry_decision_fee_rate_source.v1",
            "fee_bps_by_instrument_id": {
                "condition-token-up.POLYMARKET": 2.5,
                "condition-token-down.POLYMARKET": 3.5
            }
        }),
    );
    let fee_rate_source_sha256 = sha256_file_for_cli_test(&fee_rate_source_path);

    Command::new(env!("CARGO_BIN_EXE_bolt-v2"))
        .args([
            "operator-artifacts",
            "collect-pre-run-funding-margin-source",
            "--config",
            config_path.to_str().expect("config path should be utf-8"),
            "--strategy-instance-id",
            "configured_updown_main",
            "--fee-rate-source",
            fee_rate_source_path
                .to_str()
                .expect("fee source path should be utf-8"),
            "--fee-rate-source-sha256",
            &fee_rate_source_sha256,
            "--max-fee-rate-source-bytes",
            "100000",
            "--output",
            output_path.to_str().expect("output path should be utf-8"),
        ])
        .env("AWS_ENDPOINT_URL_SSM", &ssm_url)
        .env("AWS_ACCESS_KEY_ID", "fake-access-key")
        .env("AWS_SECRET_ACCESS_KEY", "fake-secret-key")
        .env("AWS_REGION", "eu-west-1")
        .env("AWS_MAX_ATTEMPTS", "1")
        .output()
        .expect("bolt-v3 funding margin source collection should run")
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

fn spawn_one_shot_date_server(date: &'static str) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("test HTTP server should bind");
    let address = listener.local_addr().expect("test HTTP server address");
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("test HTTP request should connect");
        let mut request = [0_u8; 1024];
        let _ = stream.read(&mut request);
        let response = format!(
            "HTTP/1.1 200 OK\r\nDate: {date}\r\nContent-Length: 2\r\nConnection: close\r\n\r\nOK"
        );
        stream
            .write_all(response.as_bytes())
            .expect("test HTTP response should write");
    });
    format!("http://{address}")
}

fn spawn_one_shot_clob_balance_allowance_server(
    balance: &'static str,
    allowance: &'static str,
) -> (String, mpsc::Receiver<String>) {
    spawn_clob_balance_allowance_server_with_bodies([balance_allowance_body(balance, allowance)])
}

fn spawn_one_shot_clob_balance_allowance_update_server() -> (String, mpsc::Receiver<String>) {
    spawn_clob_balance_allowance_server_with_bodies(["{}".to_string()])
}

fn spawn_clob_balance_allowance_server_with_bodies(
    bodies: impl IntoIterator<Item = String>,
) -> (String, mpsc::Receiver<String>) {
    spawn_clob_balance_allowance_server_with_statuses(bodies.into_iter().map(|body| (200, body)))
}

fn spawn_clob_balance_allowance_server_with_statuses(
    responses: impl IntoIterator<Item = (u16, String)>,
) -> (String, mpsc::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("test CLOB server should bind");
    let address = listener.local_addr().expect("test CLOB server address");
    let responses: Vec<(u16, String)> = responses.into_iter().collect();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        for (status, body) in responses {
            let (mut stream, _) = listener.accept().expect("test CLOB request should connect");
            let request = read_test_http_request(&mut stream);
            tx.send(request)
                .expect("test CLOB request should report to caller");
            let reason = match status {
                200 => "OK",
                500 => "Internal Server Error",
                _ => "Test Status",
            };
            let response = format!(
                "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream
                .write_all(response.as_bytes())
                .expect("test CLOB response should write");
        }
    });
    (format!("http://{address}"), rx)
}

fn spawn_eth_call_server_with_results(
    results: impl IntoIterator<Item = &'static str>,
) -> (String, mpsc::Receiver<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("test JSON-RPC server should bind");
    let address = listener.local_addr().expect("test JSON-RPC server address");
    let results: Vec<&'static str> = results.into_iter().collect();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut requests = Vec::new();
        for result in results {
            let (mut stream, _) = listener
                .accept()
                .expect("test JSON-RPC request should connect");
            requests.push(read_test_http_request(&mut stream));
            let body = format!(r#"{{"jsonrpc":"2.0","id":1,"result":"{result}"}}"#);
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream
                .write_all(response.as_bytes())
                .expect("test JSON-RPC response should write");
        }
        tx.send(requests)
            .expect("test JSON-RPC requests should report to caller");
    });
    (format!("http://{address}"), rx)
}

fn balance_allowance_body(balance: &str, allowance: &str) -> String {
    format!(r#"{{"balance":"{balance}","allowance":"{allowance}"}}"#)
}

fn spawn_one_shot_clob_open_orders_server() -> (String, mpsc::Receiver<String>) {
    spawn_clob_open_orders_server_with_bodies([empty_open_orders_body()])
}

fn spawn_clob_open_orders_server_with_bodies(
    bodies: impl IntoIterator<Item = String>,
) -> (String, mpsc::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("test CLOB server should bind");
    let address = listener.local_addr().expect("test CLOB server address");
    let bodies: Vec<String> = bodies.into_iter().collect();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        for body in bodies {
            let (mut stream, _) = listener.accept().expect("test CLOB request should connect");
            let request = read_test_http_request(&mut stream);
            tx.send(request)
                .expect("test CLOB request should report to caller");
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream
                .write_all(response.as_bytes())
                .expect("test CLOB response should write");
        }
    });
    (format!("http://{address}"), rx)
}

fn empty_open_orders_body() -> String {
    r#"{"data":[],"next_cursor":"LTE="}"#.to_string()
}

fn open_orders_body_with_one_live_order() -> String {
    r#"{"data":[{
        "associate_trades":["0xabc001"],
        "id":"0x1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef12",
        "status":"LIVE",
        "market":"0xdd22472e552920b8438158ea7238bfadfa4f736aa4cee91a6b86c39ead110917",
        "original_size":"100.0000",
        "outcome":"Yes",
        "maker_address":"0xf39fd6e51aad88f6f4ce6ab8827279cfffb92266",
        "owner":"00000000-0000-0000-0000-000000000001",
        "price":"0.5000",
        "side":"BUY",
        "size_matched":"25.0000",
        "asset_id":"71321045679252212594626385532706912750332728571942532289631379312455583992563",
        "expiration":null,
        "order_type":"GTC",
        "created_at":1703875200
    }],"next_cursor":"LTE="}"#
        .to_string()
}

fn spawn_one_shot_data_api_positions_server() -> (String, mpsc::Receiver<String>) {
    spawn_one_shot_data_api_positions_server_with_body("[]")
}

fn spawn_one_shot_data_api_positions_server_with_body(
    body: impl Into<String>,
) -> (String, mpsc::Receiver<String>) {
    spawn_data_api_positions_server_with_bodies([body.into()])
}

fn spawn_data_api_positions_server_with_bodies(
    bodies: impl IntoIterator<Item = String>,
) -> (String, mpsc::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("test Data API server should bind");
    let address = listener.local_addr().expect("test Data API server address");
    let bodies: Vec<String> = bodies.into_iter().collect();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        for body in bodies {
            let (mut stream, _) = listener
                .accept()
                .expect("test Data API request should connect");
            let request = read_test_http_request(&mut stream);
            tx.send(request)
                .expect("test Data API request should report to caller");
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream
                .write_all(response.as_bytes())
                .expect("test Data API response should write");
        }
    });
    (format!("http://{address}"), rx)
}

fn spawn_fake_ssm_server(
    values: BTreeMap<&'static str, &'static str>,
) -> (String, mpsc::Receiver<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("test SSM server should bind");
    let address = listener.local_addr().expect("test SSM server address");
    let values: BTreeMap<String, String> = values
        .into_iter()
        .map(|(path, value)| (path.to_string(), value.to_string()))
        .collect();
    let expected_request_count = values.len();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut paths = Vec::new();
        for _ in 0..expected_request_count {
            let (mut stream, _) = listener.accept().expect("test SSM request should connect");
            let request = read_test_http_request(&mut stream);
            let body = request
                .split("\r\n\r\n")
                .nth(1)
                .expect("test SSM request should include JSON body");
            let value: serde_json::Value =
                serde_json::from_str(body).expect("test SSM request body should parse");
            let name = value["Name"]
                .as_str()
                .expect("test SSM request should include Name")
                .to_string();
            paths.push(name.clone());
            let secret_value = values
                .get(&name)
                .unwrap_or_else(|| panic!("unexpected SSM path {name}"));
            let response_body = serde_json::json!({
                "Parameter": {
                    "Name": name,
                    "Type": "SecureString",
                    "Value": secret_value,
                }
            })
            .to_string();
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/x-amz-json-1.1\r\nx-amzn-RequestId: test-request\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                response_body.len(),
                response_body
            );
            stream
                .write_all(response.as_bytes())
                .expect("test SSM response should write");
        }
        tx.send(paths).expect("test SSM paths should report");
    });
    (format!("http://{address}"), rx)
}

fn read_test_http_request(stream: &mut std::net::TcpStream) -> String {
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("test request read timeout should set");
    let mut bytes = [0_u8; 8192];
    let size = stream
        .read(&mut bytes)
        .expect("test HTTP request should read");
    String::from_utf8_lossy(&bytes[..size]).to_string()
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
fn bolt_v3_cli_rejects_legacy_generic_entry_decision_collectors() {
    for command in [
        "collect-entry-decision-source-inputs",
        "collect-entry-decision-proof-sources",
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_bolt-v2"))
            .args(["operator-artifacts", command, "--help"])
            .output()
            .unwrap_or_else(|error| panic!("bolt-v3 {command} help should run: {error}"));

        assert!(
            !output.status.success(),
            "generic entry-decision collector must not expose legacy provider-shaped flags: {command}"
        );
    }
}

#[test]
fn bolt_v3_cli_exposes_collect_reference_quote_observations_source() {
    let output = Command::new(env!("CARGO_BIN_EXE_bolt-v2"))
        .args([
            "operator-artifacts",
            "collect-reference-quote-observations-source",
            "--help",
        ])
        .output()
        .expect("bolt-v3 reference quote observations source help should run");

    assert!(
        output.status.success(),
        "expected reference quote observations source help to pass, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("--config"));
    assert!(stdout.contains("--strategy-instance-id"));
    assert!(stdout.contains("--output"));
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
            "configured_updown_main",
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
        "/bolt/polymarket/private-key",
        "/bolt/polymarket/api-key",
        "/bolt/polymarket/api-secret",
        "/bolt/polymarket/api-passphrase",
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
fn bolt_v3_base_static_operator_artifacts_command_succeeds_without_blocker_manifest() {
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
            "generate-base-static",
            "--config",
            config_path.to_str().expect("fixture path should be utf-8"),
            "--output-dir",
            output_dir.to_str().expect("output path should be utf-8"),
            "--strategy-instance-id",
            "configured_updown_main",
        ])
        .output()
        .expect("bolt-v3 base static operator artifacts command should run");

    assert!(
        output.status.success(),
        "expected base static command to pass, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stdout_json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("stdout summary should be JSON");
    assert_eq!(
        stdout_json
            .as_object()
            .expect("stdout should be an object")
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["generated_artifacts"]
    );
    for artifact_name in [
        "ssm-manifest.json",
        "financial-envelope.json",
        "approval-nonce.json",
    ] {
        assert!(
            output_dir.join(artifact_name).exists(),
            "base static command should write {artifact_name}"
        );
        assert!(
            stdout.contains(artifact_name),
            "stdout should report generated artifact {artifact_name}: {stdout}"
        );
    }
    for artifact_name in [
        "static-artifacts-manifest.json",
        "strategy-input.json",
        "pre-run-state.json",
        "abort-plan.json",
    ] {
        assert!(
            !output_dir.join(artifact_name).exists(),
            "base static command must not write blocked artifact {artifact_name}"
        );
    }
    for forbidden in [
        "/bolt/polymarket/private-key",
        "/bolt/polymarket/api-key",
        "/bolt/polymarket/api-secret",
        "/bolt/polymarket/api-passphrase",
        "nonce_bytes",
        "nonce_material",
    ] {
        assert!(
            !stdout.contains(forbidden),
            "stdout must not expose raw secret path or nonce material {forbidden}"
        );
    }
}

#[test]
fn bolt_v3_secrets_check_rejects_missing_provider_secret_field() {
    let config_path = write_bolt_v3_fixture_root(|root| {
        root.replace(
            "api_secret_ssm_path = \"/bolt/polymarket/api-secret\"\n",
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
        !stderr.contains("/bolt/polymarket/api-secret"),
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

fn write_cli_json_artifact(
    dir: &Path,
    file_name: &str,
    value: serde_json::Value,
) -> std::path::PathBuf {
    let path = dir.join(file_name);
    fs::write(
        &path,
        serde_json::to_vec_pretty(&value).expect("test JSON should encode"),
    )
    .expect("test JSON artifact should write");
    path
}

fn sha256_file_for_cli_test(path: &Path) -> String {
    hex::encode(Sha256::digest(
        fs::read(path).expect("test artifact should read"),
    ))
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

fn live_canary_with_egress_identity_toml(
    observed_identity_path: &str,
    approved_egress_identity_sha256: &str,
) -> String {
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
egress_identity_observed_path = "{}"
egress_identity_observed_max_bytes = 1024
approved_egress_identity_sha256 = "{}"
"#,
        toml_string(observed_identity_path),
        toml_string(approved_egress_identity_sha256),
    )
}

fn live_canary_with_operator_evidence_toml(evidence: &LiveCanaryOperatorEvidenceBlock) -> String {
    // Round-trip the blocker-B binding fields verbatim so the reloaded config
    // re-derives the identical approval envelope hash the CLI emitted. Both are
    // `Option<String>`; emit the line only when `Some` so a `None` round-trips
    // back to `None`.
    let expected_gate_session_sha256_line = match evidence.expected_gate_session_sha256.as_deref() {
        Some(value) => format!(
            "expected_gate_session_sha256 = \"{}\"\n",
            toml_string(value)
        ),
        None => String::new(),
    };
    let canary_proof_order_intent_sha256_line =
        match evidence.canary_proof_order_intent_sha256.as_deref() {
            Some(value) => format!(
                "canary_proof_order_intent_sha256 = \"{}\"\n",
                toml_string(value)
            ),
            None => String::new(),
        };
    let no_submit_readiness_report_sha256_line =
        match evidence.no_submit_readiness_report_sha256.as_deref() {
            Some(value) => format!(
                "no_submit_readiness_report_sha256 = \"{}\"\n",
                toml_string(value)
            ),
            None => String::new(),
        };
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
{expected_gate_session_sha256_line}financial_envelope_path = "{financial_envelope_path}"
financial_envelope_sha256 = "{financial_envelope_sha256}"
pre_run_state_path = "{pre_run_state_path}"
pre_run_state_sha256 = "{pre_run_state_sha256}"
abort_plan_path = "{abort_plan_path}"
abort_plan_sha256 = "{abort_plan_sha256}"
{canary_proof_order_intent_sha256_line}{no_submit_readiness_report_sha256_line}canary_evidence_path = "{canary_evidence_path}"
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
