use sha2::{Digest, Sha256};

use bolt_v2::{
    bolt_v3_config::{LiveCanaryBlock, load_bolt_v3_config},
    bolt_v3_operator_artifacts::build_redacted_ssm_manifest,
    bolt_v3_tiny_canary_evidence::Phase8OperatorApprovalEnvelope,
};

mod support;
use support::repo_path;

#[test]
fn redacted_ssm_manifest_hashes_configured_ssm_paths_without_values() {
    let loaded = load_bolt_v3_config(&repo_path("tests/fixtures/bolt_v3/root.toml"))
        .expect("fixture config should load");

    let manifest =
        build_redacted_ssm_manifest(&loaded).expect("redacted SSM manifest should build");

    assert_eq!(manifest.schema_version, 1);
    assert_eq!(manifest.record_kind, "bolt_v3.redacted_ssm_manifest.v1");
    assert_eq!(
        manifest.config_bundle_checksum,
        loaded.config_bundle_checksum
    );
    assert_eq!(manifest.aws_region, loaded.root.aws.region);
    assert_eq!(manifest.entries.len(), 6);

    let manifest_json =
        serde_json::to_string(&manifest).expect("manifest should serialize for redaction check");
    for raw_path in [
        "/bolt/polymarket_main/private_key",
        "/bolt/polymarket_main/api_key",
        "/bolt/polymarket_main/api_secret",
        "/bolt/polymarket_main/passphrase",
        "/bolt/binance_reference/api_key",
        "/bolt/binance_reference/api_secret",
    ] {
        assert!(
            !manifest_json.contains(raw_path),
            "redacted SSM manifest must not contain raw SSM path {raw_path}"
        );
    }

    assert_manifest_entry(
        &manifest,
        "polymarket_main",
        "POLYMARKET",
        "private_key_ssm_path",
        "/bolt/polymarket_main/private_key",
    );
    assert_manifest_entry(
        &manifest,
        "polymarket_main",
        "POLYMARKET",
        "api_key_ssm_path",
        "/bolt/polymarket_main/api_key",
    );
    assert_manifest_entry(
        &manifest,
        "polymarket_main",
        "POLYMARKET",
        "api_secret_ssm_path",
        "/bolt/polymarket_main/api_secret",
    );
    assert_manifest_entry(
        &manifest,
        "polymarket_main",
        "POLYMARKET",
        "passphrase_ssm_path",
        "/bolt/polymarket_main/passphrase",
    );
    assert_manifest_entry(
        &manifest,
        "binance_reference",
        "BINANCE",
        "api_key_ssm_path",
        "/bolt/binance_reference/api_key",
    );
    assert_manifest_entry(
        &manifest,
        "binance_reference",
        "BINANCE",
        "api_secret_ssm_path",
        "/bolt/binance_reference/api_secret",
    );
}

#[test]
fn financial_envelope_from_loaded_config_is_operator_approval_schema_compatible() {
    let loaded = load_fixture_with_live_canary();
    let strategy_instance_id = loaded
        .strategies
        .first()
        .expect("fixture should load a strategy")
        .config
        .strategy_instance_id
        .as_str();

    let financial_envelope = bolt_v2::bolt_v3_operator_artifacts::build_phase8_financial_envelope(
        &loaded,
        strategy_instance_id,
    )
    .expect("financial envelope should build from loaded config");
    let financial_envelope_bytes = serde_json::to_vec_pretty(&financial_envelope)
        .expect("financial envelope should serialize");

    let temp = tempfile::tempdir().expect("tempdir should create");
    let financial_envelope_path = temp.path().join("phase8-financial-envelope.json");
    std::fs::write(&financial_envelope_path, financial_envelope_bytes)
        .expect("financial envelope should write");
    let financial_envelope_sha256 =
        Phase8OperatorApprovalEnvelope::sha256_file(&financial_envelope_path)
            .expect("financial envelope hash should compute");
    let envelope = Phase8OperatorApprovalEnvelope {
        head_sha: String::new(),
        root_toml_path: String::new(),
        root_toml_sha256: String::new(),
        approval_envelope_sha256: String::new(),
        ssm_manifest_path: String::new(),
        ssm_manifest_sha256: String::new(),
        strategy_input_evidence_path: String::new(),
        strategy_input_evidence_sha256: String::new(),
        financial_envelope_path: financial_envelope_path.to_string_lossy().to_string(),
        financial_envelope_sha256,
        pre_run_state_path: String::new(),
        pre_run_state_sha256: String::new(),
        abort_plan_path: String::new(),
        abort_plan_sha256: String::new(),
        operator_approval_id: String::new(),
        approval_not_before_unix_secs: 0,
        approval_not_after_unix_secs: 0,
        approval_nonce_path: String::new(),
        approval_nonce_sha256: String::new(),
        approval_consumption_path: String::new(),
        canary_evidence_path: String::new(),
        strategy_cancel_path: None,
    };

    assert_eq!(
        envelope
            .approved_strategy_instance_id_hash()
            .expect("operator envelope should parse helper financial envelope"),
        sha256_text(strategy_instance_id)
    );
}

#[test]
fn approval_nonce_writer_stores_hash_only_and_refuses_overwrite() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let nonce_path = temp.path().join("approval-nonce.json");

    let written = bolt_v2::bolt_v3_operator_artifacts::write_approval_nonce_artifact(&nonce_path)
        .expect("approval nonce should write once");
    let nonce_bytes = std::fs::read(&nonce_path).expect("approval nonce should read");
    assert_eq!(written.sha256, hex::encode(Sha256::digest(&nonce_bytes)));

    let nonce_json: serde_json::Value =
        serde_json::from_slice(&nonce_bytes).expect("approval nonce should parse");
    assert_eq!(nonce_json["schema_version"], 1);
    assert_eq!(
        nonce_json["record_kind"],
        "bolt_v3.operator_approval_nonce.v1"
    );
    let nonce_hash = nonce_json["nonce_sha256"]
        .as_str()
        .expect("nonce hash should be a string");
    assert_eq!(nonce_hash.len(), 64);
    assert!(
        nonce_hash.chars().all(|char| char.is_ascii_hexdigit()),
        "nonce hash should be lowercase hex"
    );
    assert_eq!(
        nonce_json
            .as_object()
            .expect("nonce should be object")
            .len(),
        3
    );
    let serialized = String::from_utf8(nonce_bytes).expect("nonce JSON should be UTF-8");
    assert!(!serialized.contains("raw"));
    assert!(!serialized.contains("nonce_bytes"));
    assert!(!serialized.contains("nonce_material"));

    let overwrite_error =
        bolt_v2::bolt_v3_operator_artifacts::write_approval_nonce_artifact(&nonce_path)
            .expect_err("approval nonce writer should refuse overwrite");
    assert!(
        overwrite_error.to_string().contains("already exists"),
        "overwrite error should mention existing file: {overwrite_error}"
    );
}

#[test]
fn abort_plan_writer_fails_closed_when_static_prerequisites_are_unproven() {
    let loaded = load_fixture_with_live_canary();
    let strategy_instance_id = loaded
        .strategies
        .first()
        .expect("fixture should load a strategy")
        .config
        .strategy_instance_id
        .as_str();
    let temp = tempfile::tempdir().expect("tempdir should create");
    let abort_plan_path = temp.path().join("abort-plan.json");

    let error = bolt_v2::bolt_v3_operator_artifacts::write_abort_plan_artifact(
        &loaded,
        strategy_instance_id,
        &abort_plan_path,
    )
    .expect_err("abort plan should fail closed until panic gate and service policy are proven");

    assert!(
        error.to_string().contains("panic gate and service policy"),
        "abort blocker should cite missing panic gate evidence: {error}"
    );
    assert!(
        !abort_plan_path.exists(),
        "failed abort plan generation must not leave a success artifact"
    );
}

fn load_fixture_with_live_canary() -> bolt_v2::bolt_v3_config::LoadedBoltV3Config {
    let mut loaded = load_bolt_v3_config(&repo_path("tests/fixtures/bolt_v3/root.toml"))
        .expect("fixture config should load");
    loaded.root.live_canary = Some(LiveCanaryBlock {
        approval_id: "test-operator-approval".to_string(),
        no_submit_readiness_report_path: "reports/no-submit-readiness.json".to_string(),
        max_no_submit_readiness_report_bytes: 1_000_000,
        readiness_report_max_age_seconds: 300,
        reference_quote_max_age_seconds: 30,
        reference_quote_wait_timeout_seconds: 5,
        reference_quote_probe_actor_id: "test-reference-probe".to_string(),
        reference_quote_probe_log_events: false,
        reference_quote_probe_log_commands: false,
        max_live_order_count: 1,
        max_notional_per_order: loaded.root.risk.default_max_notional_per_order.clone(),
        operator_evidence: None,
    });
    loaded
}

fn assert_manifest_entry(
    manifest: &bolt_v2::bolt_v3_operator_artifacts::BoltV3RedactedSsmManifest,
    client_key: &str,
    provider_key: &str,
    field_name: &str,
    ssm_path: &str,
) {
    let entry = manifest
        .entries
        .iter()
        .find(|entry| {
            entry.client_key == client_key
                && entry.provider_key == provider_key
                && entry.field_name == field_name
        })
        .expect("expected redacted SSM manifest entry");
    assert_eq!(entry.ssm_path_sha256, sha256_text(ssm_path));
}

fn sha256_text(value: &str) -> String {
    hex::encode(Sha256::digest(value.as_bytes()))
}
