mod support;

use bolt_v2::{
    bolt_v3_config::{LiveCanaryBlock, LoadedBoltV3Config, load_bolt_v3_config},
    bolt_v3_live_node::{build_bolt_v3_live_node, run_bolt_v3_live_node},
    bolt_v3_tiny_canary_evidence::{
        PHASE8_BLOCKED_BEFORE_LIVE_RUNNER_RUN_ID, Phase8CanaryEvidence, Phase8CanaryEvidenceInput,
        Phase8EvidenceRef, Phase8LiveCanaryResultRefs, Phase8LiveOrderRef,
        Phase8OperatorApprovalEnvelope, Phase8RuntimeCaptureRef, Phase8StrategyInputSafetyAudit,
        evaluate_phase8_canary_preflight, phase8_required_env, phase8_sha256_text,
    },
    nt_runtime_capture::spool_root_for_instance,
};
use rust_decimal::Decimal;
use serde::Deserialize;
use std::env;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const PHASE8_TEST_PRICE_TO_BEAT_SOURCE: &str = "chainlink_data_streams.report_at_boundary";
const PHASE8_VALIDATION_HEAD_SHA: &str = "expected-head";
const PHASE8_VALIDATION_ROOT_TOML_SHA256: &str = "expected-config-hash";
const PHASE8_TEST_APPROVAL_ENVELOPE_SHA256: &str =
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const PHASE8_TEST_CLIENT_ORDER_ID_HASH: &str =
    "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";
const PHASE8_TEST_VENUE_ORDER_ID_HASH: &str =
    "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff";
const PHASE8_OPERATOR_APPROVAL_ID: &str = "operator-approved-canary-001";
const PHASE8_VALIDATION_UNIX_SECS: i64 = 1_500;

#[test]
fn phase8_operator_harness_is_ignored_and_uses_production_runner_shape() {
    let source = support::repo_text("tests/bolt_v3_tiny_canary_operator.rs");

    assert!(source.contains("#[ignore]"));
    assert!(source.contains("Phase8OperatorApprovalEnvelope::from_env"));
    assert!(source.contains("validate_approved_evidence_against"));
    assert!(source.contains("consume_approval_after_live_runner_entry_validation"));
    assert!(source.contains("evaluate_phase8_canary_preflight"));
    assert!(source.contains("write_json_file"));
    assert!(source.contains("build_bolt_v3_live_node"));
    assert!(source.contains("run_bolt_v3_live_node"));
    assert!(source.contains("tokio::task::LocalSet"));
    assert!(!source.contains(&format!(
        "{}{}{}",
        "BOLT_V3_PHASE8_", "CURRENT_HEAD", "_SHA"
    )));
    assert!(!source.contains(&format!("{}{}", "LiveNode", "::run")));
    assert!(!source.contains(&format!("{}{}", ".submit", "_order(")));
    assert!(!source.contains(&format!("{}{}", ".cancel", "_order(")));
    assert!(!source.contains(&format!("{}{}", ".replace", "_order(")));
}

#[test]
fn phase8_operator_harness_does_not_block_before_production_runner() {
    let source = support::repo_text("tests/bolt_v3_tiny_canary_operator.rs");

    assert!(!source.contains(&format!("{}{}", "LiveProof", "CaptureUnavailable")));
    assert!(!source.contains(&format!(
        "{}{}",
        "phase8_live_runner_requires_", "post_run_evidence_capture"
    )));
}

#[test]
fn phase8_operator_harness_derives_strategy_audit_from_evidence_file() {
    let source = support::repo_text("tests/bolt_v3_tiny_canary_operator.rs");

    assert!(source.contains("envelope.approved_price_to_beat_source()?"));
    assert!(source.contains("Phase8StrategyInputSafetyAudit::from_evidence_file"));
    let harness_start = source
        .rfind("async fn phase8_operator_harness_requires_exact_approval_before_live_runner")
        .expect("operator harness start should exist");
    let harness = &source[harness_start..];
    let source_index = harness
        .find("let approved_price_to_beat_source = envelope.approved_price_to_beat_source()?")
        .expect("operator harness should derive approved price source");
    let audit_index = harness
        .find("let strategy_audit = Phase8StrategyInputSafetyAudit::from_evidence_file")
        .expect("operator harness should parse strategy input evidence");
    let validation_index = harness
        .find("envelope.validate_approved_evidence_against")
        .expect("operator harness should validate approval");
    let consumption_index = harness
        .find("envelope.consume_approval_after_live_runner_entry_validation")
        .expect("operator harness should consume approval");
    assert!(source_index < audit_index);
    assert!(audit_index < validation_index);
    assert!(validation_index < consumption_index);
    assert!(!source.contains(&format!(
        "{}{}",
        "Phase8StrategyInputSafetyAudit::", "approved()"
    )));
}

#[test]
fn phase8_operator_harness_prevalidates_success_evidence_before_runner() {
    let source = support::repo_text("tests/bolt_v3_tiny_canary_operator.rs");
    let start = source
        .rfind("async fn phase8_operator_harness_requires_exact_approval_before_live_runner")
        .expect("operator harness start should exist");
    let end = source[start..]
        .find("\nfn phase8_current_checkout_head_sha")
        .map(|offset| start + offset)
        .expect("operator harness end should exist");
    let harness = &source[start..end];

    let input_index = harness
        .find("let evidence_input = phase8_operator_evidence_input")
        .expect("success evidence input should be prepared before live runner");
    let snapshot_index = harness
        .find("snapshot_before_run")
        .expect("post-run evidence paths should be snapshotted before live runner");
    let runner_index = harness
        .find("run_bolt_v3_live_node")
        .expect("operator harness should use production live runner");

    assert!(input_index < runner_index);
    assert!(snapshot_index < runner_index);
}

#[test]
fn phase8_operator_harness_consumes_approval_after_entry_validation() {
    let source = support::repo_text("tests/bolt_v3_tiny_canary_operator.rs");
    let start = source
        .rfind("async fn phase8_operator_harness_requires_exact_approval_before_live_runner")
        .expect("operator harness start should exist");
    let end = source[start..]
        .find("\nfn phase8_current_checkout_head_sha")
        .map(|offset| start + offset)
        .expect("operator harness end should exist");
    let harness = &source[start..end];

    let preflight_index = harness
        .find("let preflight = evaluate_phase8_canary_preflight")
        .expect("operator harness should evaluate preflight");
    let result_paths_index = harness
        .find("let result_paths = Phase8OperatorLiveResultPaths::from_env()?")
        .expect("operator harness should load live result paths");
    let path_binding_index = harness
        .find("result_paths.assert_belongs_to_runtime_capture")
        .expect("operator harness should bind live result paths to runtime capture");
    let snapshot_index = harness
        .find("let pre_run_snapshot = result_paths.snapshot_before_run()?")
        .expect("operator harness should snapshot result paths");
    let consumption_index = harness
        .find("envelope.consume_approval_after_live_runner_entry_validation")
        .expect("operator harness should consume approval");
    let runner_index = harness
        .find("run_bolt_v3_live_node")
        .expect("operator harness should use production live runner");

    assert!(preflight_index < result_paths_index);
    assert!(result_paths_index < path_binding_index);
    assert!(path_binding_index < snapshot_index);
    assert!(snapshot_index < consumption_index);
    assert!(consumption_index < runner_index);
}

#[test]
fn phase8_operator_harness_binds_live_proof_to_runtime_admission_and_spool() {
    let source = support::repo_text("tests/bolt_v3_tiny_canary_operator.rs");

    assert!(source.contains("admitted_order_count()"));
    assert!(source.contains("spool_root_for_instance"));
    assert!(source.contains("assert_belongs_to_runtime_capture"));
    assert!(source.contains("to_refs_after_operator_post_run_proofs"));
    assert!(source.contains("assert_changed_after_run"));
    assert!(source.contains("phase8_read_operator_evidence_proof"));
    assert!(!source.contains(&format!("{}{}{}", "BOLT_V3_PHASE8_", "RUNTIME_RUN", "_ID")));
    assert!(!source.contains(&format!(
        "{}{}{}",
        "strategy_cancel_path: phase8_required_env(\"",
        "BOLT_V3_PHASE8_STRATEGY_CANCEL_PATH",
        "\")?"
    )));
}

#[test]
fn phase8_operator_harness_waits_for_post_run_proofs_after_runner() {
    let source = support::repo_text("tests/bolt_v3_tiny_canary_operator.rs");
    let start = source
        .rfind("async fn phase8_operator_harness_requires_exact_approval_before_live_runner")
        .expect("operator harness start should exist");
    let end = source[start..]
        .find("\nfn phase8_current_checkout_head_sha")
        .map(|offset| start + offset)
        .expect("operator harness end should exist");
    let harness = &source[start..end];

    let runner_index = harness
        .find("run_bolt_v3_live_node")
        .expect("operator harness should use production live runner");
    let wait_index = harness
        .find("to_refs_after_operator_post_run_proofs")
        .expect("operator harness should wait for post-run operator proofs");

    assert!(runner_index < wait_index);
    assert!(source.contains("observed_errors"));
    assert!(source.contains("observed errors"));
}

#[test]
fn phase8_operator_envelope_rejects_unopened_approval_window() {
    let fixture = Phase8OperatorEnvelopeFixture::new();

    fixture.assert_valid_baseline();
    let error = fixture
        .validate(&fixture.envelope, 999)
        .expect_err("approval before not_before should fail closed");

    assert_error_contains(&error, "not yet valid");
    fixture.assert_not_consumed();
}

#[test]
fn phase8_operator_envelope_rejects_nonce_hash_mismatch() {
    assert_invalid_phase8_operator_envelope(
        |envelope| {
            envelope.approval_nonce_sha256 = wrong_sha256();
        },
        "nonce sha256",
    );
}

#[test]
fn phase8_operator_envelope_rejects_ssm_manifest_hash_mismatch() {
    assert_invalid_phase8_operator_envelope(
        |envelope| {
            envelope.ssm_manifest_sha256 = wrong_sha256();
        },
        "ssm_manifest_sha256",
    );
}

#[test]
fn phase8_operator_envelope_rejects_strategy_input_hash_mismatch() {
    assert_invalid_phase8_operator_envelope(
        |envelope| {
            envelope.strategy_input_evidence_sha256 = wrong_sha256();
        },
        "strategy_input_evidence_sha256",
    );
}

#[test]
fn phase8_operator_envelope_rejects_financial_envelope_hash_mismatch() {
    assert_invalid_phase8_operator_envelope(
        |envelope| {
            envelope.financial_envelope_sha256 = wrong_sha256();
        },
        "financial_envelope_sha256",
    );
}

#[test]
fn phase8_operator_envelope_rejects_pre_run_state_hash_mismatch() {
    assert_invalid_phase8_operator_envelope(
        |envelope| {
            envelope.pre_run_state_sha256 = wrong_sha256();
        },
        "pre_run_state_sha256",
    );
}

fn assert_invalid_phase8_operator_envelope(
    mutate: impl FnOnce(&mut Phase8OperatorApprovalEnvelope),
    expected_error: &str,
) {
    let fixture = Phase8OperatorEnvelopeFixture::new();
    fixture.assert_valid_baseline();

    let mut envelope = fixture.envelope.clone();
    mutate(&mut envelope);
    let error = fixture
        .validate(&envelope, PHASE8_VALIDATION_UNIX_SECS)
        .expect_err("invalid operator envelope should fail closed");

    assert_error_contains(&error, expected_error);
    fixture.assert_not_consumed();
}

fn assert_error_contains(error: &anyhow::Error, expected: &str) {
    assert!(
        error.to_string().contains(expected),
        "error should mention `{expected}`: {error}"
    );
}

struct Phase8OperatorEnvelopeFixture {
    _temp: tempfile::TempDir,
    loaded: LoadedBoltV3Config,
    envelope: Phase8OperatorApprovalEnvelope,
}

impl Phase8OperatorEnvelopeFixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().expect("tempdir should create");
        let manifest_path = temp.path().join("phase8-ssm-manifest.json");
        std::fs::write(
            &manifest_path,
            r#"{"ssm_paths":["/bolt-v3/test/private-key"]}"#,
        )
        .expect("manifest should write");
        let manifest_hash = Phase8OperatorApprovalEnvelope::sha256_file(&manifest_path)
            .expect("manifest hash should compute");

        let strategy_input_path = temp.path().join("phase8-strategy-input-evidence.json");
        write_phase8_operator_strategy_input(&strategy_input_path);
        let strategy_input_hash = Phase8OperatorApprovalEnvelope::sha256_file(&strategy_input_path)
            .expect("strategy input evidence hash should compute");

        let financial_envelope_path = temp.path().join("phase8-financial-envelope.json");
        write_phase8_operator_financial_envelope(&financial_envelope_path);
        let financial_envelope_hash =
            Phase8OperatorApprovalEnvelope::sha256_file(&financial_envelope_path)
                .expect("financial envelope hash should compute");

        let pre_run_state_path = temp.path().join("phase8-pre-run-state.json");
        write_phase8_operator_pre_run_state(&pre_run_state_path);
        let pre_run_state_hash = Phase8OperatorApprovalEnvelope::sha256_file(&pre_run_state_path)
            .expect("pre-run state hash should compute");

        let abort_plan_path = temp.path().join("phase8-abort-plan.json");
        write_phase8_operator_abort_plan(&abort_plan_path);
        let abort_plan_hash = Phase8OperatorApprovalEnvelope::sha256_file(&abort_plan_path)
            .expect("abort plan hash should compute");

        let approval_nonce_path = temp.path().join("phase8-approval-nonce.json");
        std::fs::write(
            &approval_nonce_path,
            r#"{"record_kind":"phase8_operator_approval_nonce","nonce_hash":"dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"}"#,
        )
        .expect("approval nonce should write");
        let approval_nonce_hash = Phase8OperatorApprovalEnvelope::sha256_file(&approval_nonce_path)
            .expect("approval nonce hash should compute");
        let approval_consumption_path = temp.path().join("phase8-approval-consumed.json");
        let root_toml_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
        let loaded = phase8_loaded_with_operator_canary("reports/no-submit-readiness.json");

        Self {
            envelope: Phase8OperatorApprovalEnvelope {
                head_sha: PHASE8_VALIDATION_HEAD_SHA.to_string(),
                root_toml_path: root_toml_path.to_string_lossy().to_string(),
                root_toml_sha256: PHASE8_VALIDATION_ROOT_TOML_SHA256.to_string(),
                approval_envelope_sha256: PHASE8_TEST_APPROVAL_ENVELOPE_SHA256.to_string(),
                ssm_manifest_path: manifest_path.to_string_lossy().to_string(),
                ssm_manifest_sha256: manifest_hash,
                strategy_input_evidence_path: strategy_input_path.to_string_lossy().to_string(),
                strategy_input_evidence_sha256: strategy_input_hash,
                financial_envelope_path: financial_envelope_path.to_string_lossy().to_string(),
                financial_envelope_sha256: financial_envelope_hash,
                pre_run_state_path: pre_run_state_path.to_string_lossy().to_string(),
                pre_run_state_sha256: pre_run_state_hash,
                abort_plan_path: abort_plan_path.to_string_lossy().to_string(),
                abort_plan_sha256: abort_plan_hash,
                operator_approval_id: PHASE8_OPERATOR_APPROVAL_ID.to_string(),
                approval_not_before_unix_secs: 1_000,
                approval_not_after_unix_secs: 2_000,
                approval_nonce_path: approval_nonce_path.to_string_lossy().to_string(),
                approval_nonce_sha256: approval_nonce_hash,
                approval_consumption_path: approval_consumption_path.to_string_lossy().to_string(),
                canary_evidence_path: temp
                    .path()
                    .join("phase8-canary-evidence.json")
                    .to_string_lossy()
                    .to_string(),
                strategy_cancel_path: phase8_live_canary_strategy_cancel_path(&loaded),
                client_order_id_hash: PHASE8_TEST_CLIENT_ORDER_ID_HASH.to_string(),
                venue_order_id_hash: PHASE8_TEST_VENUE_ORDER_ID_HASH.to_string(),
            },
            loaded,
            _temp: temp,
        }
    }

    fn validate(
        &self,
        envelope: &Phase8OperatorApprovalEnvelope,
        current_unix_secs: i64,
    ) -> anyhow::Result<()> {
        envelope.validate_approved_evidence_against(
            PHASE8_VALIDATION_HEAD_SHA,
            PHASE8_VALIDATION_ROOT_TOML_SHA256,
            PHASE8_OPERATOR_APPROVAL_ID,
            &self.loaded,
            current_unix_secs,
        )
    }

    fn assert_valid_baseline(&self) {
        self.validate(&self.envelope, PHASE8_VALIDATION_UNIX_SECS)
            .expect("valid operator envelope fixture should pass full validation");
    }

    fn assert_not_consumed(&self) {
        assert!(
            !Path::new(&self.envelope.approval_consumption_path).exists(),
            "rejected envelope validation must not create consumption evidence"
        );
    }
}

fn phase8_loaded_with_operator_canary(report_path: &str) -> LoadedBoltV3Config {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    loaded.root.live_canary = Some(LiveCanaryBlock {
        approval_id: PHASE8_OPERATOR_APPROVAL_ID.to_string(),
        no_submit_readiness_report_path: report_path.to_string(),
        max_no_submit_readiness_report_bytes: 4096,
        readiness_report_max_age_seconds: 60,
        reference_quote_max_age_seconds: 10,
        reference_quote_wait_timeout_seconds: 10,
        reference_quote_probe_actor_id: "no-submit-reference-quote-probe".to_string(),
        reference_quote_probe_log_events: true,
        reference_quote_probe_log_commands: true,
        operator_evidence: Some(support::valid_live_canary_operator_evidence()),
        max_live_order_count: 1,
        max_notional_per_order: "0.25".to_string(),
    });
    loaded
}

fn phase8_live_canary_strategy_cancel_path(loaded: &LoadedBoltV3Config) -> Option<String> {
    loaded
        .root
        .live_canary
        .as_ref()
        .and_then(|live_canary| live_canary.operator_evidence.as_ref())
        .and_then(|operator_evidence| operator_evidence.strategy_cancel_path.clone())
}

fn write_phase8_operator_strategy_input(path: &Path) {
    std::fs::write(
        path,
        r#"{"realized_volatility":"2.5","seconds_to_market_end":300,"spot_price":"100000.0","price_to_beat_value":"100000.0","expected_edge_basis_points":"12.5","worst_case_edge_basis_points":"12.5","fee_rate_basis_points":"0","price_to_beat_source":"chainlink_data_streams.report_at_boundary","reference_quote_ts_event":1234567890,"pricing_kurtosis":"0","theta_decay_factor":"0","theta_scaled_min_edge_bps":"12.5","market_selection_timestamp_ms":1234567890,"market_selection_outcome":"current","polymarket_condition_id":"condition-1","polymarket_market_slug":"btc-updown-5m","polymarket_question_id":"question-1","up_instrument_id":"condition-1-UP.POLYMARKET","down_instrument_id":"condition-1-DOWN.POLYMARKET","selected_market_observed_timestamp_ms":1234567890,"polymarket_market_start_timestamp_ms":1234567000,"polymarket_market_end_timestamp_ms":1234867000}"#,
    )
    .expect("strategy input evidence should write");
}

fn write_phase8_operator_financial_envelope(path: &Path) {
    let json = serde_json::json!({
        "max_live_order_count": 1,
        "max_notional_per_order": "0.25",
        "strategy_instance_id": "bitcoin_updown_main",
        "execution_client_id": "polymarket_main",
        "configured_target_id": "btc_updown_5m",
        "target_kind": "rotating_market",
        "rotating_market_family": "updown",
        "underlying_asset": "BTC",
        "cadence_secs": 300,
        "market_selection_rule": "active_or_next",
        "retry_interval_secs": 5,
        "blocked_after_secs": 60,
        "price_to_beat_source": PHASE8_TEST_PRICE_TO_BEAT_SOURCE,
        "edge_threshold_basis_points": 100,
        "order_notional_target": "5.00",
        "maximum_position_notional": "10.00",
        "book_impact_cap_bps": 50,
        "entry_order_type": "limit",
        "entry_time_in_force": "fok",
        "entry_is_post_only": false,
        "entry_is_reduce_only": false,
        "entry_is_quote_quantity": false,
        "exit_order_type": "market",
        "exit_time_in_force": "ioc",
        "exit_is_post_only": false,
        "exit_is_reduce_only": false,
        "exit_is_quote_quantity": false
    });
    std::fs::write(
        path,
        serde_json::to_vec(&json).expect("financial envelope should serialize"),
    )
    .expect("financial envelope should write");
}

fn write_phase8_operator_pre_run_state(path: &Path) {
    let evidence_hash = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let json = serde_json::json!({
        "execution_client_id": "polymarket_main",
        "configured_target_id": "btc_updown_5m",
        "host_clock_skew_within_bound": true,
        "host_clock_skew_evidence_hash": evidence_hash,
        "conflicting_open_orders_absent": true,
        "preexisting_position_absent": true,
        "venue_account_state_evidence_hash": evidence_hash,
        "market_state_approved": true,
        "market_window_approved": true,
        "market_state_evidence_hash": evidence_hash,
        "funding_margin_covers_max_notional_plus_fees": true,
        "funding_margin_evidence_hash": evidence_hash,
        "single_runner_lock_acquired": true,
        "single_runner_lock_evidence_hash": evidence_hash,
        "egress_identity_approved": true,
        "egress_identity_evidence_hash": evidence_hash,
        "clob_v2_adapter_signing_verified": true,
        "clob_v2_adapter_signing_evidence_hash": evidence_hash,
        "clob_v2_collateral_accounting_verified": true,
        "clob_v2_collateral_accounting_evidence_hash": evidence_hash,
        "clob_v2_fee_behavior_verified": true,
        "clob_v2_fee_behavior_evidence_hash": evidence_hash,
        "release_manifest_clob_signing_version": "clob_v2",
        "release_manifest_nt_revision_matches_compiled_pin": true,
        "release_manifest_evidence_hash": evidence_hash
    });
    std::fs::write(
        path,
        serde_json::to_vec(&json).expect("pre-run state should serialize"),
    )
    .expect("pre-run state should write");
}

fn write_phase8_operator_abort_plan(path: &Path) {
    let json = serde_json::json!({
        "execution_client_id": "polymarket_main",
        "configured_target_id": "btc_updown_5m",
        "cancel_if_open_defined": true,
        "nt_accepted_venue_pending_abort_defined": true,
        "partial_fill_abort_defined": true,
        "network_partition_during_submit_abort_defined": true,
        "panic_gate_trip_abort_defined": true
    });
    std::fs::write(
        path,
        serde_json::to_vec(&json).expect("abort plan should serialize"),
    )
    .expect("abort plan should write");
}

fn wrong_sha256() -> String {
    "e".repeat(64)
}

#[test]
fn live_result_paths_reject_stale_restart_reconciliation_evidence() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let run_id = "phase8-live-run-001";
    let client_order_id_hash = "c".repeat(64);
    let venue_order_id_hash = "d".repeat(64);
    let scanned_hash = "e".repeat(64);
    let retention_hash = "f".repeat(64);
    let decision_path = temp.path().join("decision.json");
    let nt_submit_path = temp.path().join("nt-submit.json");
    let venue_state_path = temp.path().join("venue-state.json");
    let restart_path = temp.path().join("restart.json");
    let post_hygiene_path = temp.path().join("post-hygiene.json");
    let paths = Phase8OperatorLiveResultPaths {
        decision_evidence_path: decision_path.to_string_lossy().to_string(),
        client_order_id_hash: client_order_id_hash.clone(),
        venue_order_id_hash: venue_order_id_hash.clone(),
        nt_submit_event_path: nt_submit_path.to_string_lossy().to_string(),
        venue_order_state_path: venue_state_path.to_string_lossy().to_string(),
        strategy_cancel_path: None,
        restart_reconciliation_path: restart_path.to_string_lossy().to_string(),
        post_run_hygiene_path: post_hygiene_path.to_string_lossy().to_string(),
    };

    write_json_proof(&decision_path, serde_json::json!({"record_kind": "old"}));
    write_json_proof(&nt_submit_path, serde_json::json!({"record_kind": "old"}));
    write_json_proof(&venue_state_path, serde_json::json!({"record_kind": "old"}));
    write_json_proof(
        &restart_path,
        serde_json::json!({
            "record_kind": "restart_reconciliation",
            "source_run_id": run_id,
            "strategy_instance_id_hash": phase8_sha256_text("bitcoin_updown_main"),
            "client_order_id_hash": client_order_id_hash,
            "venue_order_id_hash": venue_order_id_hash,
            "venue_order_outcome": "filled",
            "order_remains_open": false
        }),
    );
    write_json_proof(
        &post_hygiene_path,
        serde_json::json!({"record_kind": "old"}),
    );
    let snapshot = paths
        .snapshot_before_run()
        .expect("pre-run snapshot should hash existing proof files");

    write_json_proof(
        &decision_path,
        serde_json::json!({
            "record_kind": "decision_evidence",
            "run_id": run_id,
            "strategy_instance_id_hash": phase8_sha256_text("bitcoin_updown_main"),
            "client_order_id_hash": client_order_id_hash
        }),
    );
    write_json_proof(
        &nt_submit_path,
        serde_json::json!({
            "record_kind": "nt_submit_event",
            "run_id": run_id,
            "strategy_instance_id_hash": phase8_sha256_text("bitcoin_updown_main"),
            "client_order_id_hash": client_order_id_hash
        }),
    );
    write_json_proof(
        &venue_state_path,
        serde_json::json!({
            "record_kind": "venue_order_state",
            "run_id": run_id,
            "strategy_instance_id_hash": phase8_sha256_text("bitcoin_updown_main"),
            "client_order_id_hash": client_order_id_hash,
            "venue_order_id_hash": venue_order_id_hash
        }),
    );
    write_json_proof(
        &post_hygiene_path,
        serde_json::json!({
            "record_kind": "post_run_hygiene",
            "run_id": run_id,
            "strategy_instance_id_hash": phase8_sha256_text("bitcoin_updown_main"),
            "client_order_id_hash": client_order_id_hash,
            "venue_order_id_hash": venue_order_id_hash,
            "raw_secret_residue_absent": true,
            "scanned_artifact_hashes": [scanned_hash],
            "retention_purge_path_hash": retention_hash
        }),
    );

    let error = paths
        .to_refs(
            &snapshot,
            run_id,
            &phase8_sha256_text("bitcoin_updown_main"),
        )
        .expect_err("stale restart reconciliation evidence must fail");

    assert!(
        error.to_string().contains("restart reconciliation"),
        "error should mention stale restart reconciliation evidence: {error}"
    );
}

#[test]
fn live_result_paths_reject_restart_reconciliation_outside_runtime_capture() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let spool_root = temp.path().join("runtime-spool");
    let outside_root = temp.path().join("operator-written");
    let paths = Phase8OperatorLiveResultPaths {
        decision_evidence_path: spool_root
            .join("decision.json")
            .to_string_lossy()
            .to_string(),
        client_order_id_hash: "c".repeat(64),
        venue_order_id_hash: "d".repeat(64),
        nt_submit_event_path: spool_root
            .join("nt-submit.json")
            .to_string_lossy()
            .to_string(),
        venue_order_state_path: spool_root
            .join("venue-state.json")
            .to_string_lossy()
            .to_string(),
        strategy_cancel_path: None,
        restart_reconciliation_path: outside_root
            .join("restart.json")
            .to_string_lossy()
            .to_string(),
        post_run_hygiene_path: spool_root
            .join("post-hygiene.json")
            .to_string_lossy()
            .to_string(),
    };

    let error = paths
        .assert_belongs_to_runtime_capture(&spool_root.to_string_lossy())
        .expect_err("restart reconciliation evidence outside runtime capture must fail");

    assert!(
        error.to_string().contains("restart reconciliation"),
        "error should mention restart reconciliation evidence path: {error}"
    );
}

#[test]
fn live_result_paths_reject_decision_evidence_outside_runtime_capture() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let spool_root = temp.path().join("runtime-spool");
    let outside_root = temp.path().join("operator-written");
    let paths = Phase8OperatorLiveResultPaths {
        decision_evidence_path: outside_root
            .join("decision.json")
            .to_string_lossy()
            .to_string(),
        client_order_id_hash: "c".repeat(64),
        venue_order_id_hash: "d".repeat(64),
        nt_submit_event_path: spool_root
            .join("nt-submit.json")
            .to_string_lossy()
            .to_string(),
        venue_order_state_path: spool_root
            .join("venue-state.json")
            .to_string_lossy()
            .to_string(),
        strategy_cancel_path: None,
        restart_reconciliation_path: spool_root
            .join("restart.json")
            .to_string_lossy()
            .to_string(),
        post_run_hygiene_path: spool_root
            .join("post-hygiene.json")
            .to_string_lossy()
            .to_string(),
    };

    let error = paths
        .assert_belongs_to_runtime_capture(&spool_root.to_string_lossy())
        .expect_err("decision evidence outside runtime capture must fail");

    assert!(
        error.to_string().contains("decision evidence"),
        "error should mention decision evidence path: {error}"
    );
}

#[test]
fn live_result_paths_require_strategy_cancel_when_venue_order_remains_open() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let run_id = "phase8-live-run-001";
    let client_order_id_hash = "c".repeat(64);
    let venue_order_id_hash = "d".repeat(64);
    let scanned_hash = "e".repeat(64);
    let retention_hash = "f".repeat(64);
    let decision_path = temp.path().join("decision.json");
    let nt_submit_path = temp.path().join("nt-submit.json");
    let venue_state_path = temp.path().join("venue-state.json");
    let restart_path = temp.path().join("restart.json");
    let post_hygiene_path = temp.path().join("post-hygiene.json");
    let paths = Phase8OperatorLiveResultPaths {
        decision_evidence_path: decision_path.to_string_lossy().to_string(),
        client_order_id_hash: client_order_id_hash.clone(),
        venue_order_id_hash: venue_order_id_hash.clone(),
        nt_submit_event_path: nt_submit_path.to_string_lossy().to_string(),
        venue_order_state_path: venue_state_path.to_string_lossy().to_string(),
        strategy_cancel_path: None,
        restart_reconciliation_path: restart_path.to_string_lossy().to_string(),
        post_run_hygiene_path: post_hygiene_path.to_string_lossy().to_string(),
    };

    write_json_proof(&decision_path, serde_json::json!({"record_kind": "old"}));
    write_json_proof(&nt_submit_path, serde_json::json!({"record_kind": "old"}));
    write_json_proof(&venue_state_path, serde_json::json!({"record_kind": "old"}));
    write_json_proof(&restart_path, serde_json::json!({"record_kind": "old"}));
    write_json_proof(
        &post_hygiene_path,
        serde_json::json!({"record_kind": "old"}),
    );
    let snapshot = paths
        .snapshot_before_run()
        .expect("pre-run snapshot should hash existing proof files");

    write_json_proof(
        &decision_path,
        serde_json::json!({
            "record_kind": "decision_evidence",
            "run_id": run_id,
            "strategy_instance_id_hash": phase8_sha256_text("bitcoin_updown_main"),
            "client_order_id_hash": client_order_id_hash
        }),
    );
    write_json_proof(
        &nt_submit_path,
        serde_json::json!({
            "record_kind": "nt_submit_event",
            "run_id": run_id,
            "strategy_instance_id_hash": phase8_sha256_text("bitcoin_updown_main"),
            "client_order_id_hash": client_order_id_hash
        }),
    );
    write_json_proof(
        &venue_state_path,
        serde_json::json!({
            "record_kind": "venue_order_state",
            "run_id": run_id,
            "strategy_instance_id_hash": phase8_sha256_text("bitcoin_updown_main"),
            "client_order_id_hash": client_order_id_hash,
            "venue_order_id_hash": venue_order_id_hash,
            "venue_order_outcome": "accepted",
            "order_remains_open": true
        }),
    );
    write_json_proof(
        &restart_path,
        serde_json::json!({
            "record_kind": "restart_reconciliation",
            "source_run_id": run_id,
            "strategy_instance_id_hash": phase8_sha256_text("bitcoin_updown_main"),
            "client_order_id_hash": client_order_id_hash,
            "venue_order_id_hash": venue_order_id_hash,
            "venue_order_outcome": "filled",
            "order_remains_open": false
        }),
    );
    write_json_proof(
        &post_hygiene_path,
        serde_json::json!({
            "record_kind": "post_run_hygiene",
            "run_id": run_id,
            "strategy_instance_id_hash": phase8_sha256_text("bitcoin_updown_main"),
            "client_order_id_hash": client_order_id_hash,
            "venue_order_id_hash": venue_order_id_hash,
            "raw_secret_residue_absent": true,
            "scanned_artifact_hashes": [scanned_hash],
            "retention_purge_path_hash": retention_hash
        }),
    );

    let error = paths
        .to_refs(
            &snapshot,
            run_id,
            &phase8_sha256_text("bitcoin_updown_main"),
        )
        .expect_err("open venue order must require strategy cancel evidence");

    assert!(
        error.to_string().contains("strategy cancel"),
        "error should mention missing strategy cancel evidence: {error}"
    );
}

#[test]
fn live_result_paths_reject_terminal_venue_outcome_marked_open() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let run_id = "phase8-live-run-001";
    let client_order_id_hash = "c".repeat(64);
    let venue_order_id_hash = "d".repeat(64);
    let scanned_hash = "e".repeat(64);
    let retention_hash = "f".repeat(64);
    let decision_path = temp.path().join("decision.json");
    let nt_submit_path = temp.path().join("nt-submit.json");
    let venue_state_path = temp.path().join("venue-state.json");
    let cancel_path = temp.path().join("cancel.json");
    let restart_path = temp.path().join("restart.json");
    let post_hygiene_path = temp.path().join("post-hygiene.json");
    let paths = Phase8OperatorLiveResultPaths {
        decision_evidence_path: decision_path.to_string_lossy().to_string(),
        client_order_id_hash: client_order_id_hash.clone(),
        venue_order_id_hash: venue_order_id_hash.clone(),
        nt_submit_event_path: nt_submit_path.to_string_lossy().to_string(),
        venue_order_state_path: venue_state_path.to_string_lossy().to_string(),
        strategy_cancel_path: Some(cancel_path.to_string_lossy().to_string()),
        restart_reconciliation_path: restart_path.to_string_lossy().to_string(),
        post_run_hygiene_path: post_hygiene_path.to_string_lossy().to_string(),
    };

    write_json_proof(&decision_path, serde_json::json!({"record_kind": "old"}));
    write_json_proof(&nt_submit_path, serde_json::json!({"record_kind": "old"}));
    write_json_proof(&venue_state_path, serde_json::json!({"record_kind": "old"}));
    write_json_proof(&cancel_path, serde_json::json!({"record_kind": "old"}));
    write_json_proof(&restart_path, serde_json::json!({"record_kind": "old"}));
    write_json_proof(
        &post_hygiene_path,
        serde_json::json!({"record_kind": "old"}),
    );
    let snapshot = paths
        .snapshot_before_run()
        .expect("pre-run snapshot should hash existing proof files");

    write_json_proof(
        &decision_path,
        serde_json::json!({
            "record_kind": "decision_evidence",
            "run_id": run_id,
            "strategy_instance_id_hash": phase8_sha256_text("bitcoin_updown_main"),
            "client_order_id_hash": client_order_id_hash
        }),
    );
    write_json_proof(
        &nt_submit_path,
        serde_json::json!({
            "record_kind": "nt_submit_event",
            "run_id": run_id,
            "strategy_instance_id_hash": phase8_sha256_text("bitcoin_updown_main"),
            "client_order_id_hash": client_order_id_hash
        }),
    );
    write_json_proof(
        &venue_state_path,
        serde_json::json!({
            "record_kind": "venue_order_state",
            "run_id": run_id,
            "strategy_instance_id_hash": phase8_sha256_text("bitcoin_updown_main"),
            "client_order_id_hash": client_order_id_hash,
            "venue_order_id_hash": venue_order_id_hash,
            "venue_order_outcome": "filled",
            "order_remains_open": true
        }),
    );
    write_json_proof(
        &cancel_path,
        serde_json::json!({
            "record_kind": "strategy_cancel",
            "run_id": run_id,
            "strategy_instance_id_hash": phase8_sha256_text("bitcoin_updown_main"),
            "client_order_id_hash": client_order_id_hash,
            "venue_order_id_hash": venue_order_id_hash
        }),
    );
    write_json_proof(
        &restart_path,
        serde_json::json!({
            "record_kind": "restart_reconciliation",
            "source_run_id": run_id,
            "strategy_instance_id_hash": phase8_sha256_text("bitcoin_updown_main"),
            "client_order_id_hash": client_order_id_hash,
            "venue_order_id_hash": venue_order_id_hash,
            "venue_order_outcome": "filled",
            "order_remains_open": false
        }),
    );
    write_json_proof(
        &post_hygiene_path,
        serde_json::json!({
            "record_kind": "post_run_hygiene",
            "run_id": run_id,
            "strategy_instance_id_hash": phase8_sha256_text("bitcoin_updown_main"),
            "client_order_id_hash": client_order_id_hash,
            "venue_order_id_hash": venue_order_id_hash,
            "raw_secret_residue_absent": true,
            "scanned_artifact_hashes": [scanned_hash],
            "retention_purge_path_hash": retention_hash
        }),
    );

    let error = paths
        .to_refs(
            &snapshot,
            run_id,
            &phase8_sha256_text("bitcoin_updown_main"),
        )
        .expect_err("terminal venue outcome must not be marked open");

    assert!(
        error.to_string().contains("order_remains_open"),
        "error should mention inconsistent venue open state: {error}"
    );
}

#[test]
fn live_result_paths_reject_open_restart_reconciliation() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let run_id = "phase8-live-run-001";
    let client_order_id_hash = "c".repeat(64);
    let venue_order_id_hash = "d".repeat(64);
    let scanned_hash = "e".repeat(64);
    let retention_hash = "f".repeat(64);
    let decision_path = temp.path().join("decision.json");
    let nt_submit_path = temp.path().join("nt-submit.json");
    let venue_state_path = temp.path().join("venue-state.json");
    let restart_path = temp.path().join("restart.json");
    let post_hygiene_path = temp.path().join("post-hygiene.json");
    let paths = Phase8OperatorLiveResultPaths {
        decision_evidence_path: decision_path.to_string_lossy().to_string(),
        client_order_id_hash: client_order_id_hash.clone(),
        venue_order_id_hash: venue_order_id_hash.clone(),
        nt_submit_event_path: nt_submit_path.to_string_lossy().to_string(),
        venue_order_state_path: venue_state_path.to_string_lossy().to_string(),
        strategy_cancel_path: None,
        restart_reconciliation_path: restart_path.to_string_lossy().to_string(),
        post_run_hygiene_path: post_hygiene_path.to_string_lossy().to_string(),
    };

    write_json_proof(&decision_path, serde_json::json!({"record_kind": "old"}));
    write_json_proof(&nt_submit_path, serde_json::json!({"record_kind": "old"}));
    write_json_proof(&venue_state_path, serde_json::json!({"record_kind": "old"}));
    write_json_proof(&restart_path, serde_json::json!({"record_kind": "old"}));
    write_json_proof(
        &post_hygiene_path,
        serde_json::json!({"record_kind": "old"}),
    );
    let snapshot = paths
        .snapshot_before_run()
        .expect("pre-run snapshot should hash existing proof files");

    write_json_proof(
        &decision_path,
        serde_json::json!({
            "record_kind": "decision_evidence",
            "run_id": run_id,
            "strategy_instance_id_hash": phase8_sha256_text("bitcoin_updown_main"),
            "client_order_id_hash": client_order_id_hash
        }),
    );
    write_json_proof(
        &nt_submit_path,
        serde_json::json!({
            "record_kind": "nt_submit_event",
            "run_id": run_id,
            "strategy_instance_id_hash": phase8_sha256_text("bitcoin_updown_main"),
            "client_order_id_hash": client_order_id_hash
        }),
    );
    write_json_proof(
        &venue_state_path,
        serde_json::json!({
            "record_kind": "venue_order_state",
            "run_id": run_id,
            "strategy_instance_id_hash": phase8_sha256_text("bitcoin_updown_main"),
            "client_order_id_hash": client_order_id_hash,
            "venue_order_id_hash": venue_order_id_hash,
            "venue_order_outcome": "filled",
            "order_remains_open": false
        }),
    );
    write_json_proof(
        &restart_path,
        serde_json::json!({
            "record_kind": "restart_reconciliation",
            "source_run_id": run_id,
            "strategy_instance_id_hash": phase8_sha256_text("bitcoin_updown_main"),
            "client_order_id_hash": client_order_id_hash,
            "venue_order_id_hash": venue_order_id_hash,
            "venue_order_outcome": "filled",
            "order_remains_open": true
        }),
    );
    write_json_proof(
        &post_hygiene_path,
        serde_json::json!({
            "record_kind": "post_run_hygiene",
            "run_id": run_id,
            "strategy_instance_id_hash": phase8_sha256_text("bitcoin_updown_main"),
            "client_order_id_hash": client_order_id_hash,
            "venue_order_id_hash": venue_order_id_hash,
            "raw_secret_residue_absent": true,
            "scanned_artifact_hashes": [scanned_hash],
            "retention_purge_path_hash": retention_hash
        }),
    );

    let error = paths
        .to_refs(
            &snapshot,
            run_id,
            &phase8_sha256_text("bitcoin_updown_main"),
        )
        .expect_err("open restart reconciliation evidence must fail");

    assert!(
        error.to_string().contains("restart reconciliation"),
        "error should mention restart reconciliation evidence: {error}"
    );
    assert!(
        error.to_string().contains("order_remains_open"),
        "error should mention open restart state: {error}"
    );
}

fn write_json_proof(path: &Path, value: serde_json::Value) {
    std::fs::write(
        path,
        serde_json::to_vec(&value).expect("proof should serialize"),
    )
    .expect("proof should write");
}

#[test]
fn live_result_paths_reject_unapproved_post_run_hygiene_strategy_hash() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let run_id = "phase8-live-run-001";
    let approved_strategy_hash = phase8_sha256_text("bitcoin_updown_main");
    let unapproved_strategy_hash = phase8_sha256_text("bitcoin_updown_secondary");
    let client_order_id_hash = "c".repeat(64);
    let venue_order_id_hash = "d".repeat(64);
    let scanned_hash = "e".repeat(64);
    let retention_hash = "f".repeat(64);
    let decision_path = temp.path().join("decision.json");
    let nt_submit_path = temp.path().join("nt-submit.json");
    let venue_state_path = temp.path().join("venue-state.json");
    let restart_path = temp.path().join("restart.json");
    let post_hygiene_path = temp.path().join("post-hygiene.json");
    let paths = Phase8OperatorLiveResultPaths {
        decision_evidence_path: decision_path.to_string_lossy().to_string(),
        client_order_id_hash: client_order_id_hash.clone(),
        venue_order_id_hash: venue_order_id_hash.clone(),
        nt_submit_event_path: nt_submit_path.to_string_lossy().to_string(),
        venue_order_state_path: venue_state_path.to_string_lossy().to_string(),
        strategy_cancel_path: None,
        restart_reconciliation_path: restart_path.to_string_lossy().to_string(),
        post_run_hygiene_path: post_hygiene_path.to_string_lossy().to_string(),
    };

    write_json_proof(&decision_path, serde_json::json!({"record_kind": "old"}));
    write_json_proof(&nt_submit_path, serde_json::json!({"record_kind": "old"}));
    write_json_proof(&venue_state_path, serde_json::json!({"record_kind": "old"}));
    write_json_proof(&restart_path, serde_json::json!({"record_kind": "old"}));
    write_json_proof(
        &post_hygiene_path,
        serde_json::json!({"record_kind": "old"}),
    );
    let snapshot = paths
        .snapshot_before_run()
        .expect("pre-run snapshot should hash existing proof files");

    write_json_proof(
        &decision_path,
        serde_json::json!({
            "record_kind": "decision_evidence",
            "run_id": run_id,
            "strategy_instance_id_hash": approved_strategy_hash,
            "client_order_id_hash": client_order_id_hash
        }),
    );
    write_json_proof(
        &nt_submit_path,
        serde_json::json!({
            "record_kind": "nt_submit_event",
            "run_id": run_id,
            "strategy_instance_id_hash": approved_strategy_hash,
            "client_order_id_hash": client_order_id_hash
        }),
    );
    write_json_proof(
        &venue_state_path,
        serde_json::json!({
            "record_kind": "venue_order_state",
            "run_id": run_id,
            "strategy_instance_id_hash": approved_strategy_hash,
            "client_order_id_hash": client_order_id_hash,
            "venue_order_id_hash": venue_order_id_hash,
            "venue_order_outcome": "filled",
            "order_remains_open": false
        }),
    );
    write_json_proof(
        &restart_path,
        serde_json::json!({
            "record_kind": "restart_reconciliation",
            "source_run_id": run_id,
            "strategy_instance_id_hash": approved_strategy_hash,
            "client_order_id_hash": client_order_id_hash,
            "venue_order_id_hash": venue_order_id_hash,
            "venue_order_outcome": "filled",
            "order_remains_open": false
        }),
    );
    write_json_proof(
        &post_hygiene_path,
        serde_json::json!({
            "record_kind": "post_run_hygiene",
            "run_id": run_id,
            "strategy_instance_id_hash": unapproved_strategy_hash,
            "client_order_id_hash": client_order_id_hash,
            "venue_order_id_hash": venue_order_id_hash,
            "raw_secret_residue_absent": true,
            "scanned_artifact_hashes": [scanned_hash],
            "retention_purge_path_hash": retention_hash
        }),
    );

    let error = paths
        .to_refs(&snapshot, run_id, &approved_strategy_hash)
        .expect_err("unapproved post-run hygiene strategy proof must fail");

    assert!(
        error.to_string().contains("strategy_instance_id_hash"),
        "error should mention strategy hash mismatch: {error}"
    );
}

#[test]
fn live_result_paths_reject_unapproved_strategy_hash() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let run_id = "phase8-live-run-001";
    let approved_strategy_hash = phase8_sha256_text("bitcoin_updown_main");
    let unapproved_strategy_hash = phase8_sha256_text("bitcoin_updown_secondary");
    let client_order_id_hash = "c".repeat(64);
    let venue_order_id_hash = "d".repeat(64);
    let scanned_hash = "e".repeat(64);
    let retention_hash = "f".repeat(64);
    let decision_path = temp.path().join("decision.json");
    let nt_submit_path = temp.path().join("nt-submit.json");
    let venue_state_path = temp.path().join("venue-state.json");
    let restart_path = temp.path().join("restart.json");
    let post_hygiene_path = temp.path().join("post-hygiene.json");
    let paths = Phase8OperatorLiveResultPaths {
        decision_evidence_path: decision_path.to_string_lossy().to_string(),
        client_order_id_hash: client_order_id_hash.clone(),
        venue_order_id_hash: venue_order_id_hash.clone(),
        nt_submit_event_path: nt_submit_path.to_string_lossy().to_string(),
        venue_order_state_path: venue_state_path.to_string_lossy().to_string(),
        strategy_cancel_path: None,
        restart_reconciliation_path: restart_path.to_string_lossy().to_string(),
        post_run_hygiene_path: post_hygiene_path.to_string_lossy().to_string(),
    };

    write_json_proof(&decision_path, serde_json::json!({"record_kind": "old"}));
    write_json_proof(&nt_submit_path, serde_json::json!({"record_kind": "old"}));
    write_json_proof(&venue_state_path, serde_json::json!({"record_kind": "old"}));
    write_json_proof(&restart_path, serde_json::json!({"record_kind": "old"}));
    write_json_proof(
        &post_hygiene_path,
        serde_json::json!({"record_kind": "old"}),
    );
    let snapshot = paths
        .snapshot_before_run()
        .expect("pre-run snapshot should hash existing proof files");

    write_json_proof(
        &decision_path,
        serde_json::json!({
            "record_kind": "decision_evidence",
            "run_id": run_id,
            "strategy_instance_id_hash": unapproved_strategy_hash,
            "client_order_id_hash": client_order_id_hash
        }),
    );
    write_json_proof(
        &nt_submit_path,
        serde_json::json!({
            "record_kind": "nt_submit_event",
            "run_id": run_id,
            "strategy_instance_id_hash": unapproved_strategy_hash,
            "client_order_id_hash": client_order_id_hash
        }),
    );
    write_json_proof(
        &venue_state_path,
        serde_json::json!({
            "record_kind": "venue_order_state",
            "run_id": run_id,
            "strategy_instance_id_hash": unapproved_strategy_hash,
            "client_order_id_hash": client_order_id_hash,
            "venue_order_id_hash": venue_order_id_hash,
            "venue_order_outcome": "filled",
            "order_remains_open": false
        }),
    );
    write_json_proof(
        &restart_path,
        serde_json::json!({
            "record_kind": "restart_reconciliation",
            "source_run_id": run_id,
            "strategy_instance_id_hash": unapproved_strategy_hash,
            "client_order_id_hash": client_order_id_hash,
            "venue_order_id_hash": venue_order_id_hash,
            "venue_order_outcome": "filled",
            "order_remains_open": false
        }),
    );
    write_json_proof(
        &post_hygiene_path,
        serde_json::json!({
            "record_kind": "post_run_hygiene",
            "run_id": run_id,
            "strategy_instance_id_hash": phase8_sha256_text("bitcoin_updown_main"),
            "client_order_id_hash": client_order_id_hash,
            "venue_order_id_hash": venue_order_id_hash,
            "raw_secret_residue_absent": true,
            "scanned_artifact_hashes": [scanned_hash],
            "retention_purge_path_hash": retention_hash
        }),
    );

    let error = paths
        .to_refs(&snapshot, run_id, &approved_strategy_hash)
        .expect_err("unapproved strategy result proof must fail");

    assert!(
        error.to_string().contains("strategy_instance_id_hash"),
        "error should mention strategy hash mismatch: {error}"
    );
}

#[test]
fn phase8_post_run_hygiene_proof_requires_secret_scan_and_retention() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let proof_path = temp.path().join("post-run-hygiene.json");
    let client_hash = "a".repeat(64);
    let venue_hash = "b".repeat(64);
    let scanned_hash = "c".repeat(64);
    let retention_hash = "d".repeat(64);

    std::fs::write(
        &proof_path,
        serde_json::to_vec(&serde_json::json!({
            "record_kind": "post_run_hygiene",
            "run_id": "phase8-run-001",
            "strategy_instance_id_hash": phase8_sha256_text("bitcoin_updown_main"),
            "client_order_id_hash": client_hash,
            "venue_order_id_hash": venue_hash
        }))
        .expect("proof should serialize"),
    )
    .expect("proof should write");
    let missing_scan_error = phase8_assert_post_run_hygiene_proof(
        proof_path.to_str().expect("proof path should be utf8"),
        "phase8-run-001",
        &phase8_sha256_text("bitcoin_updown_main"),
        &client_hash,
        &venue_hash,
    )
    .expect_err("missing secret scan field should fail");
    assert!(
        missing_scan_error
            .to_string()
            .contains("raw_secret_residue_absent"),
        "error should mention missing scan field: {missing_scan_error}"
    );

    std::fs::write(
        &proof_path,
        serde_json::to_vec(&serde_json::json!({
            "record_kind": "post_run_hygiene",
            "run_id": "phase8-run-001",
            "strategy_instance_id_hash": phase8_sha256_text("bitcoin_updown_main"),
            "client_order_id_hash": client_hash,
            "venue_order_id_hash": venue_hash,
            "raw_secret_residue_absent": false,
            "scanned_artifact_hashes": [scanned_hash],
            "retention_purge_path_hash": retention_hash
        }))
        .expect("proof should serialize"),
    )
    .expect("proof should write");
    let residue_error = phase8_assert_post_run_hygiene_proof(
        proof_path.to_str().expect("proof path should be utf8"),
        "phase8-run-001",
        &phase8_sha256_text("bitcoin_updown_main"),
        &client_hash,
        &venue_hash,
    )
    .expect_err("positive secret residue scan should fail");
    assert!(
        residue_error
            .to_string()
            .contains("raw_secret_residue_absent"),
        "error should mention failed scan field: {residue_error}"
    );

    std::fs::write(
        &proof_path,
        serde_json::to_vec(&serde_json::json!({
            "record_kind": "post_run_hygiene",
            "run_id": "phase8-run-001",
            "strategy_instance_id_hash": phase8_sha256_text("bitcoin_updown_main"),
            "client_order_id_hash": client_hash,
            "venue_order_id_hash": venue_hash,
            "raw_secret_residue_absent": true,
            "scanned_artifact_hashes": [scanned_hash],
            "retention_purge_path_hash": retention_hash
        }))
        .expect("proof should serialize"),
    )
    .expect("proof should write");
    phase8_assert_post_run_hygiene_proof(
        proof_path.to_str().expect("proof path should be utf8"),
        "phase8-run-001",
        &phase8_sha256_text("bitcoin_updown_main"),
        &client_hash,
        &venue_hash,
    )
    .expect("secret scan and retention proof should pass");
}

#[test]
fn phase8_operator_head_is_resolved_from_checkout() -> anyhow::Result<()> {
    let head = phase8_current_checkout_head_sha()?;

    assert_eq!(head.len(), 40);
    assert!(head.chars().all(|byte| byte.is_ascii_hexdigit()));
    Ok(())
}

// `#[rustfmt::skip]` pins the source layout of this async fn because the
// self-reflective sibling tests (`phase8_operator_harness_derives_strategy_audit_from_evidence_file`,
// `phase8_operator_harness_consumes_approval_after_entry_validation`) scan this fn's source bytes
// for specific substrings (e.g. `envelope.consume_approval_after_live_runner_entry_validation`).
// Future `cargo fmt` runs would otherwise reflow multi-line method chains and break those scans.
#[rustfmt::skip]
#[tokio::test(flavor = "current_thread")]
#[ignore]
async fn phase8_operator_harness_requires_exact_approval_before_live_runner() -> anyhow::Result<()>
{
    let envelope = Phase8OperatorApprovalEnvelope::from_env()?;
    let loaded = load_bolt_v3_config(std::path::Path::new(&envelope.root_toml_path))?;
    let root_hash = Phase8OperatorApprovalEnvelope::sha256_file(&envelope.root_toml_path)?;
    let current_head = phase8_current_checkout_head_sha()?;
    let current_unix_secs = phase8_current_unix_secs()?;
    let approved_price_to_beat_source = envelope.approved_price_to_beat_source()?;
    let strategy_audit = Phase8StrategyInputSafetyAudit::from_evidence_file(
        &envelope.strategy_input_evidence_path,
        &envelope.strategy_input_evidence_sha256,
        &approved_price_to_beat_source,
    )?;
    envelope.validate_approved_evidence_against(
        &current_head,
        &root_hash,
        loaded
            .root
            .live_canary
            .as_ref()
            .map(|block| block.approval_id.as_str())
            .unwrap_or_default(),
        &loaded,
        current_unix_secs,
    )?;
    let preflight = evaluate_phase8_canary_preflight(&loaded, &current_head, strategy_audit).await;
    if !preflight.can_enter_live_runner() {
        let blocked_runtime_capture_ref = Phase8RuntimeCaptureRef {
            spool_root_hash: phase8_sha256_text(&loaded.root.persistence.catalog_directory),
            run_id: PHASE8_BLOCKED_BEFORE_LIVE_RUNNER_RUN_ID.to_string(),
        };
        let evidence = Phase8CanaryEvidence::blocked_before_submit(
            phase8_operator_evidence_input(
                &envelope,
                &loaded,
                &root_hash,
                blocked_runtime_capture_ref,
            )?,
            preflight.block_reasons,
        );
        evidence.write_json_file(&envelope.canary_evidence_path)?;
        anyhow::bail!("phase8 canary preflight blocked before live runner");
    }
    let result_paths = Phase8OperatorLiveResultPaths::from_env()?;

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let mut node = build_bolt_v3_live_node(&loaded)?;
            let runtime_capture = phase8_operator_runtime_capture(&loaded, &node.instance_id());
            let evidence_input = phase8_operator_evidence_input(
                &envelope,
                &loaded,
                &root_hash,
                runtime_capture.reference.clone(),
            )?;
            let approved_strategy_instance_id_hash =
                evidence_input.approved_strategy_instance_id_hash.clone();
            result_paths.assert_belongs_to_runtime_capture(&runtime_capture.spool_root)?;
            let pre_run_snapshot = result_paths.snapshot_before_run()?;
            let live_runner_entry_unix_secs = phase8_current_unix_secs()?;
            envelope.consume_approval_after_live_runner_entry_validation(
                &loaded,
                live_runner_entry_unix_secs,
            )?;
            run_bolt_v3_live_node(&mut node, &loaded)
                .await
                .map_err(anyhow::Error::from)?;
            let admitted_order_count = node.admitted_order_count();
            let (decision_evidence_ref, live_order_ref, result_refs) = result_paths
                .to_refs_after_operator_post_run_proofs(
                    &pre_run_snapshot,
                    &runtime_capture.reference.run_id,
                    &loaded,
                    &approved_strategy_instance_id_hash,
                )
                .await?;
            let evidence = Phase8CanaryEvidence::live_canary_proof(
                evidence_input,
                decision_evidence_ref,
                live_order_ref,
                result_refs,
                admitted_order_count,
            )?;
            evidence.write_json_file(&envelope.canary_evidence_path)?;
            Ok::<(), anyhow::Error>(())
        })
        .await?;
    Ok(())
}

fn phase8_current_checkout_head_sha() -> anyhow::Result<String> {
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .map_err(|source| anyhow::anyhow!("failed to run git rev-parse HEAD: {source}"))?;
    if !output.status.success() {
        return Err(anyhow::anyhow!(
            "git rev-parse HEAD failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let head = String::from_utf8(output.stdout)?;
    let head = head.trim();
    if head.is_empty() {
        return Err(anyhow::anyhow!("git rev-parse HEAD returned an empty head"));
    }
    Ok(head.to_string())
}

fn phase8_current_unix_secs() -> anyhow::Result<i64> {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|source| anyhow::anyhow!("system time is before UNIX_EPOCH: {source}"))?;
    i64::try_from(duration.as_secs())
        .map_err(|source| anyhow::anyhow!("current unix seconds exceeds i64: {source}"))
}

fn phase8_operator_evidence_input(
    envelope: &Phase8OperatorApprovalEnvelope,
    loaded: &bolt_v2::bolt_v3_config::LoadedBoltV3Config,
    root_hash: &str,
    runtime_capture_ref: Phase8RuntimeCaptureRef,
) -> anyhow::Result<Phase8CanaryEvidenceInput> {
    let block = loaded
        .root
        .live_canary
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("phase8 operator evidence requires `[live_canary]`"))?;
    Ok(Phase8CanaryEvidenceInput {
        head_sha: envelope.head_sha.clone(),
        root_config_sha256: root_hash.to_string(),
        ssm_manifest_sha256: envelope.ssm_manifest_sha256.clone(),
        ssm_manifest_ref: Phase8EvidenceRef {
            path_hash: phase8_sha256_text(&envelope.ssm_manifest_path),
            record_hash: envelope.ssm_manifest_sha256.clone(),
        },
        strategy_input_evidence_ref: Phase8EvidenceRef {
            path_hash: phase8_sha256_text(&envelope.strategy_input_evidence_path),
            record_hash: envelope.strategy_input_evidence_sha256.clone(),
        },
        approved_strategy_instance_id_hash: envelope.approved_strategy_instance_id_hash()?,
        approval_id: envelope.operator_approval_id.clone(),
        max_live_order_count: block.max_live_order_count,
        max_notional_per_order: Decimal::from_str_exact(&block.max_notional_per_order)?,
        runtime_capture_ref,
    })
}

struct Phase8OperatorRuntimeCapture {
    reference: Phase8RuntimeCaptureRef,
    spool_root: String,
}

fn phase8_operator_runtime_capture(
    loaded: &bolt_v2::bolt_v3_config::LoadedBoltV3Config,
    instance_id: &str,
) -> Phase8OperatorRuntimeCapture {
    let spool_root =
        spool_root_for_instance(&loaded.root.persistence.catalog_directory, instance_id);
    Phase8OperatorRuntimeCapture {
        reference: Phase8RuntimeCaptureRef {
            spool_root_hash: phase8_sha256_text(&spool_root),
            run_id: instance_id.to_string(),
        },
        spool_root,
    }
}

fn phase8_optional_env(name: &str) -> anyhow::Result<Option<String>> {
    match env::var(name) {
        Ok(value) => {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                Ok(None)
            } else {
                Ok(Some(trimmed.to_string()))
            }
        }
        Err(env::VarError::NotPresent) => Ok(None),
        Err(error) => Err(anyhow::anyhow!(
            "failed to read phase8 env `{name}`: {error}"
        )),
    }
}

struct Phase8OperatorLiveResultPaths {
    decision_evidence_path: String,
    client_order_id_hash: String,
    venue_order_id_hash: String,
    nt_submit_event_path: String,
    venue_order_state_path: String,
    strategy_cancel_path: Option<String>,
    restart_reconciliation_path: String,
    post_run_hygiene_path: String,
}

struct Phase8OperatorLiveResultSnapshot {
    decision_evidence_sha256: Option<String>,
    nt_submit_event_sha256: Option<String>,
    venue_order_state_sha256: Option<String>,
    strategy_cancel_sha256: Option<String>,
    restart_reconciliation_sha256: Option<String>,
    post_run_hygiene_sha256: Option<String>,
}

#[derive(Deserialize)]
struct Phase8OperatorEvidenceProof {
    record_kind: String,
    run_id: Option<String>,
    source_run_id: Option<String>,
    strategy_instance_id_hash: Option<String>,
    client_order_id_hash: Option<String>,
    venue_order_id_hash: Option<String>,
    venue_order_outcome: Option<String>,
    order_remains_open: Option<bool>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Phase8PostRunHygieneProof {
    #[serde(rename = "record_kind")]
    _record_kind: String,
    #[serde(rename = "run_id")]
    _run_id: String,
    #[serde(rename = "strategy_instance_id_hash")]
    _strategy_instance_id_hash: String,
    #[serde(rename = "client_order_id_hash")]
    _client_order_id_hash: String,
    #[serde(rename = "venue_order_id_hash")]
    _venue_order_id_hash: String,
    raw_secret_residue_absent: bool,
    scanned_artifact_hashes: Vec<String>,
    retention_purge_path_hash: String,
}

impl Phase8OperatorLiveResultPaths {
    fn from_env() -> anyhow::Result<Self> {
        Ok(Self {
            decision_evidence_path: phase8_required_env("BOLT_V3_PHASE8_DECISION_EVIDENCE_PATH")?,
            client_order_id_hash: phase8_required_sha256_env(
                "BOLT_V3_PHASE8_CLIENT_ORDER_ID_HASH",
            )?,
            venue_order_id_hash: phase8_required_sha256_env("BOLT_V3_PHASE8_VENUE_ORDER_ID_HASH")?,
            nt_submit_event_path: phase8_required_env("BOLT_V3_PHASE8_NT_SUBMIT_EVENT_PATH")?,
            venue_order_state_path: phase8_required_env("BOLT_V3_PHASE8_VENUE_ORDER_STATE_PATH")?,
            strategy_cancel_path: phase8_optional_env("BOLT_V3_PHASE8_STRATEGY_CANCEL_PATH")?,
            restart_reconciliation_path: phase8_required_env(
                "BOLT_V3_PHASE8_RESTART_RECONCILIATION_PATH",
            )?,
            post_run_hygiene_path: phase8_required_env("BOLT_V3_PHASE8_POST_RUN_HYGIENE_PATH")?,
        })
    }

    fn assert_belongs_to_runtime_capture(&self, spool_root: &str) -> anyhow::Result<()> {
        phase8_assert_path_starts_with(
            &self.decision_evidence_path,
            spool_root,
            "decision evidence",
        )?;
        phase8_assert_path_starts_with(
            &self.nt_submit_event_path,
            spool_root,
            "nt submit event evidence",
        )?;
        phase8_assert_path_starts_with(
            &self.venue_order_state_path,
            spool_root,
            "venue order state evidence",
        )?;
        if let Some(strategy_cancel_path) = &self.strategy_cancel_path {
            phase8_assert_path_starts_with(
                strategy_cancel_path,
                spool_root,
                "strategy cancel evidence",
            )?;
        }
        phase8_assert_path_starts_with(
            &self.restart_reconciliation_path,
            spool_root,
            "restart reconciliation evidence",
        )?;
        phase8_assert_path_starts_with(
            &self.post_run_hygiene_path,
            spool_root,
            "post-run hygiene evidence",
        )?;
        Ok(())
    }

    fn snapshot_before_run(&self) -> anyhow::Result<Phase8OperatorLiveResultSnapshot> {
        Ok(Phase8OperatorLiveResultSnapshot {
            decision_evidence_sha256: phase8_optional_sha256_file(&self.decision_evidence_path)?,
            nt_submit_event_sha256: phase8_optional_sha256_file(&self.nt_submit_event_path)?,
            venue_order_state_sha256: phase8_optional_sha256_file(&self.venue_order_state_path)?,
            strategy_cancel_sha256: match &self.strategy_cancel_path {
                Some(strategy_cancel_path) => phase8_optional_sha256_file(strategy_cancel_path)?,
                None => None,
            },
            restart_reconciliation_sha256: phase8_optional_sha256_file(
                &self.restart_reconciliation_path,
            )?,
            post_run_hygiene_sha256: phase8_optional_sha256_file(&self.post_run_hygiene_path)?,
        })
    }

    fn assert_changed_after_run(
        &self,
        snapshot: &Phase8OperatorLiveResultSnapshot,
    ) -> anyhow::Result<()> {
        phase8_assert_changed_after_run(
            &self.decision_evidence_path,
            &snapshot.decision_evidence_sha256,
            "decision evidence",
        )?;
        phase8_assert_changed_after_run(
            &self.nt_submit_event_path,
            &snapshot.nt_submit_event_sha256,
            "nt submit event evidence",
        )?;
        phase8_assert_changed_after_run(
            &self.venue_order_state_path,
            &snapshot.venue_order_state_sha256,
            "venue order state evidence",
        )?;
        if let Some(strategy_cancel_path) = &self.strategy_cancel_path {
            phase8_assert_changed_after_run(
                strategy_cancel_path,
                &snapshot.strategy_cancel_sha256,
                "strategy cancel evidence",
            )?;
        }
        phase8_assert_changed_after_run(
            &self.restart_reconciliation_path,
            &snapshot.restart_reconciliation_sha256,
            "restart reconciliation evidence",
        )?;
        phase8_assert_changed_after_run(
            &self.post_run_hygiene_path,
            &snapshot.post_run_hygiene_sha256,
            "post-run hygiene evidence",
        )?;
        Ok(())
    }

    fn to_refs(
        &self,
        snapshot: &Phase8OperatorLiveResultSnapshot,
        run_id: &str,
        expected_strategy_instance_id_hash: &str,
    ) -> anyhow::Result<(
        Phase8EvidenceRef,
        Phase8LiveOrderRef,
        Phase8LiveCanaryResultRefs,
    )> {
        self.assert_changed_after_run(snapshot)?;
        self.assert_proof_content(run_id, expected_strategy_instance_id_hash)?;
        Ok((
            phase8_operator_evidence_ref(&self.decision_evidence_path)?,
            Phase8LiveOrderRef {
                strategy_instance_id_hash: expected_strategy_instance_id_hash.to_string(),
                client_order_id_hash: self.client_order_id_hash.clone(),
                venue_order_id_hash: self.venue_order_id_hash.clone(),
            },
            Phase8LiveCanaryResultRefs {
                nt_submit_event_ref: phase8_operator_evidence_ref(&self.nt_submit_event_path)?,
                venue_order_state_ref: phase8_operator_evidence_ref(&self.venue_order_state_path)?,
                strategy_cancel_ref: self
                    .strategy_cancel_path
                    .as_deref()
                    .map(phase8_operator_evidence_ref)
                    .transpose()?,
                restart_reconciliation_ref: phase8_operator_evidence_ref(
                    &self.restart_reconciliation_path,
                )?,
                post_run_hygiene_ref: phase8_operator_evidence_ref(&self.post_run_hygiene_path)?,
            },
        ))
    }

    async fn to_refs_after_operator_post_run_proofs(
        &self,
        snapshot: &Phase8OperatorLiveResultSnapshot,
        run_id: &str,
        loaded: &bolt_v2::bolt_v3_config::LoadedBoltV3Config,
        expected_strategy_instance_id_hash: &str,
    ) -> anyhow::Result<(
        Phase8EvidenceRef,
        Phase8LiveOrderRef,
        Phase8LiveCanaryResultRefs,
    )> {
        let wait_secs = loaded
            .root
            .nautilus
            .timeout_reconciliation_secs
            .saturating_add(loaded.root.nautilus.timeout_shutdown_secs);
        let poll_interval = Duration::from_secs(loaded.root.nautilus.timeout_shutdown_secs);
        let deadline = Instant::now() + Duration::from_secs(wait_secs);
        let mut observed_errors = Vec::new();

        loop {
            match self.to_refs(snapshot, run_id, expected_strategy_instance_id_hash) {
                Ok(refs) => return Ok(refs),
                Err(error) => {
                    observed_errors.push(error.to_string());
                    if Instant::now() >= deadline {
                        anyhow::bail!(
                            "phase8 post-run operator evidence did not become ready within nautilus.timeout_reconciliation_secs + nautilus.timeout_shutdown_secs; observed errors: {}",
                            observed_errors.join(" | ")
                        );
                    }
                }
            }

            tokio::time::sleep(poll_interval).await;
        }
    }

    fn assert_proof_content(
        &self,
        run_id: &str,
        expected_strategy_instance_id_hash: &str,
    ) -> anyhow::Result<()> {
        phase8_assert_operator_evidence_proof(
            &self.decision_evidence_path,
            "decision_evidence",
            Some(run_id),
            None,
            Some(expected_strategy_instance_id_hash),
            Some(&self.client_order_id_hash),
            None,
        )?;
        phase8_assert_operator_evidence_proof(
            &self.nt_submit_event_path,
            "nt_submit_event",
            Some(run_id),
            None,
            Some(expected_strategy_instance_id_hash),
            Some(&self.client_order_id_hash),
            None,
        )?;
        phase8_assert_operator_evidence_proof(
            &self.venue_order_state_path,
            "venue_order_state",
            Some(run_id),
            None,
            Some(expected_strategy_instance_id_hash),
            Some(&self.client_order_id_hash),
            Some(&self.venue_order_id_hash),
        )?;
        phase8_assert_venue_order_state_proof(
            &self.venue_order_state_path,
            self.strategy_cancel_path.is_some(),
        )?;
        if let Some(strategy_cancel_path) = &self.strategy_cancel_path {
            phase8_assert_operator_evidence_proof(
                strategy_cancel_path,
                "strategy_cancel",
                Some(run_id),
                None,
                Some(expected_strategy_instance_id_hash),
                Some(&self.client_order_id_hash),
                Some(&self.venue_order_id_hash),
            )?;
        }
        phase8_assert_operator_evidence_proof(
            &self.restart_reconciliation_path,
            "restart_reconciliation",
            None,
            Some(run_id),
            Some(expected_strategy_instance_id_hash),
            Some(&self.client_order_id_hash),
            Some(&self.venue_order_id_hash),
        )?;
        phase8_assert_restart_reconciliation_proof(&self.restart_reconciliation_path)?;
        phase8_assert_post_run_hygiene_proof(
            &self.post_run_hygiene_path,
            run_id,
            expected_strategy_instance_id_hash,
            &self.client_order_id_hash,
            &self.venue_order_id_hash,
        )
    }
}

fn phase8_assert_venue_order_state_proof(
    path: &str,
    strategy_cancel_present: bool,
) -> anyhow::Result<()> {
    let proof = phase8_read_operator_evidence_proof(path, "venue_order_state")?;
    let outcome = proof.venue_order_outcome.as_deref().ok_or_else(|| {
        anyhow::anyhow!("phase8 venue_order_state proof venue_order_outcome is missing")
    })?;
    match outcome {
        "accepted" | "filled" | "rejected" => {}
        _ => {
            return Err(anyhow::anyhow!(
                "phase8 venue_order_state proof venue_order_outcome must be accepted, filled, or rejected"
            ));
        }
    }
    let order_remains_open = proof.order_remains_open.ok_or_else(|| {
        anyhow::anyhow!("phase8 venue_order_state proof order_remains_open is missing")
    })?;
    if matches!(outcome, "filled" | "rejected") && order_remains_open {
        return Err(anyhow::anyhow!(
            "phase8 venue_order_state proof order_remains_open must be false for terminal outcome"
        ));
    }
    if order_remains_open && !strategy_cancel_present {
        return Err(anyhow::anyhow!(
            "phase8 venue_order_state proof requires strategy cancel evidence when order remains open"
        ));
    }
    Ok(())
}

fn phase8_assert_restart_reconciliation_proof(path: &str) -> anyhow::Result<()> {
    let proof = phase8_read_operator_evidence_proof(path, "restart_reconciliation")?;
    let outcome = proof.venue_order_outcome.as_deref().ok_or_else(|| {
        anyhow::anyhow!("phase8 restart reconciliation proof venue_order_outcome is missing")
    })?;
    match outcome {
        "filled" | "rejected" => {}
        _ => {
            return Err(anyhow::anyhow!(
                "phase8 restart reconciliation proof venue_order_outcome must be terminal"
            ));
        }
    }
    let order_remains_open = proof.order_remains_open.ok_or_else(|| {
        anyhow::anyhow!("phase8 restart reconciliation proof order_remains_open is missing")
    })?;
    if order_remains_open {
        return Err(anyhow::anyhow!(
            "phase8 restart reconciliation proof order_remains_open must be false"
        ));
    }
    Ok(())
}

fn phase8_operator_evidence_ref(path: &str) -> anyhow::Result<Phase8EvidenceRef> {
    Ok(Phase8EvidenceRef {
        path_hash: phase8_sha256_text(path),
        record_hash: Phase8OperatorApprovalEnvelope::sha256_file(path)?,
    })
}

fn phase8_required_sha256_env(name: &str) -> anyhow::Result<String> {
    let value = phase8_required_env(name)?;
    if !phase8_is_sha256_hex(&value) {
        return Err(anyhow::anyhow!(
            "required phase8 env `{name}` must be a sha256 hex digest"
        ));
    }
    Ok(value)
}

#[test]
fn phase8_sha256_shape_rejects_uppercase_hex() {
    assert!(!phase8_is_sha256_hex(&"A".repeat(64)));
}

fn phase8_is_sha256_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn phase8_assert_path_starts_with(path: &str, base: &str, label: &str) -> anyhow::Result<()> {
    phase8_reject_parent_dir(path, label)?;
    phase8_reject_parent_dir(base, "runtime capture spool root")?;
    if !Path::new(path).starts_with(Path::new(base)) {
        return Err(anyhow::anyhow!(
            "phase8 {label} path must be under runtime capture spool root"
        ));
    }
    Ok(())
}

fn phase8_reject_parent_dir(path: &str, label: &str) -> anyhow::Result<()> {
    if Path::new(path)
        .components()
        .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err(anyhow::anyhow!(
            "phase8 {label} path must not contain parent directory traversal"
        ));
    }
    Ok(())
}

fn phase8_optional_sha256_file(path: &str) -> anyhow::Result<Option<String>> {
    if Path::new(path).exists() {
        Ok(Some(Phase8OperatorApprovalEnvelope::sha256_file(path)?))
    } else {
        Ok(None)
    }
}

fn phase8_assert_changed_after_run(
    path: &str,
    before_sha256: &Option<String>,
    label: &str,
) -> anyhow::Result<()> {
    let after_sha256 = Phase8OperatorApprovalEnvelope::sha256_file(path)?;
    if before_sha256.as_ref() == Some(&after_sha256) {
        return Err(anyhow::anyhow!(
            "phase8 {label} did not change during live canary run"
        ));
    }
    Ok(())
}

fn phase8_read_operator_evidence_proof(
    path: &str,
    label: &str,
) -> anyhow::Result<Phase8OperatorEvidenceProof> {
    let file = std::fs::File::open(path)
        .map_err(|source| anyhow::anyhow!("failed to open phase8 {label} proof: {source}"))?;
    serde_json::from_reader(file)
        .map_err(|source| anyhow::anyhow!("failed to parse phase8 {label} proof: {source}"))
}

fn phase8_read_post_run_hygiene_proof(path: &str) -> anyhow::Result<Phase8PostRunHygieneProof> {
    let file = std::fs::File::open(path).map_err(|source| {
        anyhow::anyhow!("failed to open phase8 post_run_hygiene proof: {source}")
    })?;
    serde_json::from_reader(file).map_err(|source| {
        anyhow::anyhow!("failed to parse phase8 post_run_hygiene proof: {source}")
    })
}

fn phase8_assert_post_run_hygiene_proof(
    path: &str,
    expected_run_id: &str,
    expected_strategy_instance_id_hash: &str,
    expected_client_order_id_hash: &str,
    expected_venue_order_id_hash: &str,
) -> anyhow::Result<()> {
    phase8_assert_operator_evidence_proof(
        path,
        "post_run_hygiene",
        Some(expected_run_id),
        None,
        Some(expected_strategy_instance_id_hash),
        Some(expected_client_order_id_hash),
        Some(expected_venue_order_id_hash),
    )?;
    let proof = phase8_read_post_run_hygiene_proof(path)?;
    if !proof.raw_secret_residue_absent {
        return Err(anyhow::anyhow!(
            "phase8 post_run_hygiene proof raw_secret_residue_absent must be true"
        ));
    }
    if proof.scanned_artifact_hashes.is_empty() {
        return Err(anyhow::anyhow!(
            "phase8 post_run_hygiene proof scanned_artifact_hashes must not be empty"
        ));
    }
    if proof
        .scanned_artifact_hashes
        .iter()
        .any(|hash| !phase8_is_sha256_hex(hash))
    {
        return Err(anyhow::anyhow!(
            "phase8 post_run_hygiene proof scanned_artifact_hashes must contain sha256 hashes"
        ));
    }
    if !phase8_is_sha256_hex(&proof.retention_purge_path_hash) {
        return Err(anyhow::anyhow!(
            "phase8 post_run_hygiene proof retention_purge_path_hash must be a sha256 hash"
        ));
    }
    Ok(())
}

fn phase8_assert_operator_evidence_proof(
    path: &str,
    expected_kind: &str,
    expected_run_id: Option<&str>,
    expected_source_run_id: Option<&str>,
    expected_strategy_instance_id_hash: Option<&str>,
    expected_client_order_id_hash: Option<&str>,
    expected_venue_order_id_hash: Option<&str>,
) -> anyhow::Result<()> {
    let proof = phase8_read_operator_evidence_proof(path, expected_kind)?;
    if proof.record_kind != expected_kind {
        return Err(anyhow::anyhow!(
            "phase8 {expected_kind} proof has unexpected record_kind"
        ));
    }
    if let Some(expected_run_id) = expected_run_id
        && proof.run_id.as_deref() != Some(expected_run_id)
    {
        return Err(anyhow::anyhow!(
            "phase8 {expected_kind} proof run_id does not match live canary run"
        ));
    }
    if let Some(expected_source_run_id) = expected_source_run_id
        && proof.source_run_id.as_deref() != Some(expected_source_run_id)
    {
        return Err(anyhow::anyhow!(
            "phase8 {expected_kind} proof source_run_id does not match live canary run"
        ));
    }
    if let Some(expected_strategy_instance_id_hash) = expected_strategy_instance_id_hash
        && proof.strategy_instance_id_hash.as_deref() != Some(expected_strategy_instance_id_hash)
    {
        return Err(anyhow::anyhow!(
            "phase8 {expected_kind} proof strategy_instance_id_hash does not match approved financial envelope"
        ));
    }
    if let Some(expected_client_order_id_hash) = expected_client_order_id_hash
        && proof.client_order_id_hash.as_deref() != Some(expected_client_order_id_hash)
    {
        return Err(anyhow::anyhow!(
            "phase8 {expected_kind} proof client_order_id_hash does not match"
        ));
    }
    if let Some(expected_venue_order_id_hash) = expected_venue_order_id_hash
        && proof.venue_order_id_hash.as_deref() != Some(expected_venue_order_id_hash)
    {
        return Err(anyhow::anyhow!(
            "phase8 {expected_kind} proof venue_order_id_hash does not match"
        ));
    }
    Ok(())
}
