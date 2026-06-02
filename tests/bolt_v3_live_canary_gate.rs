mod support;

use bolt_v2::{
    bolt_v3_config::{
        DataClientReadinessProbeBookType, LiveCanaryBlock, LiveCanaryOperatorEvidenceBlock,
        LiveCanaryProofPolicyBlock, LiveCanaryProofTimeInForce, LoadedBoltV3Config,
        load_bolt_v3_config,
    },
    bolt_v3_live_canary_gate::{
        BoltV3LiveCanaryGateError, build_bolt_v3_live_submit_admission_report_from_config,
        check_bolt_v3_live_canary_gate, check_bolt_v3_live_canary_pre_consumption_gate,
        pre_consumption_operator_evidence_bounded_read_paths,
    },
    bolt_v3_live_node::{BoltV3LiveNodeError, build_bolt_v3_live_node_with, run_bolt_v3_live_node},
    bolt_v3_no_submit_readiness_schema::{
        APPROVAL_CONSUMPTION_RECORD_KIND, APPROVAL_CONSUMPTION_SCHEMA_VERSION,
        APPROVAL_ID_HASH_KEY, CONFIG_BUNDLE_CHECKSUM_KEY, CONTROLLED_CONNECT_STAGE,
        CONTROLLED_DISCONNECT_STAGE, EXECUTABLE_IDENTITY_KEY, GENERATED_AT_UNIX_SECONDS_KEY,
        LIVE_NODE_BUILD_STAGE, NO_SUBMIT_READINESS_SCHEMA_VERSION, OPERATOR_APPROVAL_STAGE,
        REFERENCE_READINESS_STAGE, REPORT_WRITE_STAGE, SCHEMA_VERSION_KEY, SECRET_RESOLUTION_STAGE,
        STAGE_KEY, STAGES_KEY, STATUS_KEY, STATUS_SATISFIED,
    },
};
use sha2::{Digest, Sha256};
use tokio::task::LocalSet;

const TEST_READINESS_REPORT_MAX_AGE_SECONDS: u64 = 60;

#[test]
fn run_bolt_v3_live_node_rejects_missing_live_canary_before_nt_run() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let temp = support::TempCaseDir::new("bolt-v3-live-canary-build");
    loaded.root.persistence.catalog_directory = temp.path().to_string_lossy().to_string();
    let loaded = loaded_without_live_canary(loaded);
    let mut node = build_bolt_v3_live_node_with(&loaded, |_| false, support::fake_bolt_v3_resolver)
        .expect("fixture v3 LiveNode should build");

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime should build");
    let local = LocalSet::new();

    let error = runtime.block_on(local.run_until(async {
        run_bolt_v3_live_node(&mut node, &loaded)
            .await
            .expect_err("missing live_canary block must fail before NT run")
    }));

    assert!(
        matches!(
            error,
            BoltV3LiveNodeError::LiveCanaryGate(BoltV3LiveCanaryGateError::MissingConfig)
        ),
        "expected missing live canary gate error, got {error:?}"
    );
}

#[test]
fn live_submit_admission_report_ignores_stale_operator_evidence_head_sha() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let mut operator_evidence = valid_operator_evidence();
    operator_evidence.head_sha = "0".repeat(40);
    let loaded = loaded_with_live_canary(
        loaded,
        LiveCanaryBlock {
            approval_id: "operator-approved-canary-001".to_string(),
            no_submit_readiness_report_path: "not-read-for-live-start.json".to_string(),
            max_live_order_count: 1,
            max_notional_per_order: "1.00".to_string(),
            max_no_submit_readiness_report_bytes: 4096,
            readiness_report_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_wait_timeout_seconds: 10,
            reference_quote_probe_actor_id: "no-submit-reference-quote-probe".to_string(),
            reference_quote_probe_log_events: true,
            reference_quote_probe_log_commands: true,
            egress_identity_observed_path: None,
            egress_identity_observed_max_bytes: None,
            approved_egress_identity_sha256: None,
            proof_policy: None,
            operator_evidence: Some(operator_evidence),
        },
    );

    let report = build_bolt_v3_live_submit_admission_report_from_config(&loaded)
        .expect("live start admission should not depend on exact-head operator evidence");

    assert_eq!(report.approval_id(), "operator-approved-canary-001");
    assert_eq!(report.max_live_order_count(), 1);
    assert_eq!(report.max_notional_per_order().to_string(), "1.00");
}

#[test]
fn live_submit_admission_report_ignores_no_submit_only_config_fields() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let loaded = loaded_with_live_canary(
        loaded,
        LiveCanaryBlock {
            approval_id: "operator-approved-canary-001".to_string(),
            no_submit_readiness_report_path: "".to_string(),
            max_live_order_count: 1,
            max_notional_per_order: "1.00".to_string(),
            max_no_submit_readiness_report_bytes: 0,
            readiness_report_max_age_seconds: 0,
            reference_quote_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_wait_timeout_seconds: 0,
            reference_quote_probe_actor_id: "".to_string(),
            reference_quote_probe_log_events: true,
            reference_quote_probe_log_commands: true,
            egress_identity_observed_path: None,
            egress_identity_observed_max_bytes: None,
            approved_egress_identity_sha256: None,
            proof_policy: None,
            operator_evidence: None,
        },
    );

    let report = build_bolt_v3_live_submit_admission_report_from_config(&loaded)
        .expect("live start admission should validate only runtime admission fields");

    assert_eq!(report.max_live_order_count(), 1);
    assert_eq!(report.max_notional_per_order().to_string(), "1.00");
}

#[test]
fn live_submit_admission_report_keeps_root_notional_cap() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let loaded = loaded_with_live_canary(
        loaded,
        LiveCanaryBlock {
            approval_id: "operator-approved-canary-001".to_string(),
            no_submit_readiness_report_path: "not-read-for-live-start.json".to_string(),
            max_live_order_count: 1,
            max_notional_per_order: "11.00".to_string(),
            max_no_submit_readiness_report_bytes: 4096,
            readiness_report_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_wait_timeout_seconds: 10,
            reference_quote_probe_actor_id: "no-submit-reference-quote-probe".to_string(),
            reference_quote_probe_log_events: true,
            reference_quote_probe_log_commands: true,
            egress_identity_observed_path: None,
            egress_identity_observed_max_bytes: None,
            approved_egress_identity_sha256: None,
            proof_policy: None,
            operator_evidence: None,
        },
    );

    let error = build_bolt_v3_live_submit_admission_report_from_config(&loaded)
        .expect_err("live start admission must keep the root risk notional cap");

    assert!(
        matches!(
            error,
            BoltV3LiveCanaryGateError::MaxNotionalExceedsRootRisk { .. }
        ),
        "expected root risk cap rejection, got {error:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn live_canary_gate_rejects_empty_approval_id() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let loaded = loaded_with_live_canary(
        loaded,
        LiveCanaryBlock {
            approval_id: "  ".to_string(),
            no_submit_readiness_report_path: "not-read-before-approval-check.json".to_string(),
            max_live_order_count: 1,
            max_notional_per_order: "1.00".to_string(),
            max_no_submit_readiness_report_bytes: 4096,
            readiness_report_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_wait_timeout_seconds: 10,
            reference_quote_probe_actor_id: "no-submit-reference-quote-probe".to_string(),
            reference_quote_probe_log_events: true,
            reference_quote_probe_log_commands: true,
            egress_identity_observed_path: None,
            egress_identity_observed_max_bytes: None,
            approved_egress_identity_sha256: None,
            proof_policy: None,
            operator_evidence: Some(valid_operator_evidence()),
        },
    );

    let error = check_bolt_v3_live_canary_gate(&loaded)
        .await
        .expect_err("empty approval_id must fail closed");

    assert!(
        matches!(error, BoltV3LiveCanaryGateError::MissingApprovalId),
        "expected missing approval rejection, got {error:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn live_canary_gate_rejects_empty_readiness_report_path() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let loaded = loaded_with_live_canary(
        loaded,
        LiveCanaryBlock {
            approval_id: "operator-approved-canary-001".to_string(),
            no_submit_readiness_report_path: "  ".to_string(),
            max_live_order_count: 1,
            max_notional_per_order: "1.00".to_string(),
            max_no_submit_readiness_report_bytes: 4096,
            readiness_report_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_wait_timeout_seconds: 10,
            reference_quote_probe_actor_id: "no-submit-reference-quote-probe".to_string(),
            reference_quote_probe_log_events: true,
            reference_quote_probe_log_commands: true,
            egress_identity_observed_path: None,
            egress_identity_observed_max_bytes: None,
            approved_egress_identity_sha256: None,
            proof_policy: None,
            operator_evidence: Some(valid_operator_evidence()),
        },
    );

    let error = check_bolt_v3_live_canary_gate(&loaded)
        .await
        .expect_err("empty no-submit readiness report path must fail closed");

    assert!(
        matches!(error, BoltV3LiveCanaryGateError::MissingReadinessReportPath),
        "expected missing readiness report path rejection, got {error:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn live_canary_gate_rejects_parent_dir_readiness_report_path() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let loaded = loaded_with_live_canary(
        loaded,
        LiveCanaryBlock {
            approval_id: "operator-approved-canary-001".to_string(),
            no_submit_readiness_report_path: "../no-submit-readiness.json".to_string(),
            max_live_order_count: 1,
            max_notional_per_order: "1.00".to_string(),
            max_no_submit_readiness_report_bytes: 4096,
            readiness_report_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_wait_timeout_seconds: 10,
            reference_quote_probe_actor_id: "no-submit-reference-quote-probe".to_string(),
            reference_quote_probe_log_events: true,
            reference_quote_probe_log_commands: true,
            egress_identity_observed_path: None,
            egress_identity_observed_max_bytes: None,
            approved_egress_identity_sha256: None,
            proof_policy: None,
            operator_evidence: Some(valid_operator_evidence()),
        },
    );

    let error = check_bolt_v3_live_canary_gate(&loaded)
        .await
        .expect_err("parent directory readiness report path must fail closed before report read");

    assert!(
        matches!(
            error,
            BoltV3LiveCanaryGateError::InvalidConfiguredPath {
                field: "no_submit_readiness_report_path",
                ..
            }
        ),
        "expected invalid readiness report path rejection, got {error:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn live_canary_gate_rejects_missing_operator_evidence_before_reading_report() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let loaded = loaded_with_live_canary(
        loaded,
        LiveCanaryBlock {
            approval_id: "operator-approved-canary-001".to_string(),
            no_submit_readiness_report_path: "not-read-before-operator-evidence-check.json"
                .to_string(),
            max_live_order_count: 1,
            max_notional_per_order: "1.00".to_string(),
            max_no_submit_readiness_report_bytes: 4096,
            readiness_report_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_wait_timeout_seconds: 10,
            reference_quote_probe_actor_id: "no-submit-reference-quote-probe".to_string(),
            reference_quote_probe_log_events: true,
            reference_quote_probe_log_commands: true,
            egress_identity_observed_path: None,
            egress_identity_observed_max_bytes: None,
            approved_egress_identity_sha256: None,
            proof_policy: None,
            operator_evidence: None,
        },
    );

    let error = check_bolt_v3_live_canary_gate(&loaded)
        .await
        .expect_err("missing operator_evidence must fail closed before report read");

    assert!(
        matches!(error, BoltV3LiveCanaryGateError::MissingOperatorEvidence),
        "missing operator_evidence must fail before reading report, got {error:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn live_canary_gate_rejects_missing_gate_session_binding_before_reading_report() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let mut operator_evidence = valid_operator_evidence();
    operator_evidence.gate_session_path = None;
    let loaded = loaded_with_live_canary(
        loaded,
        LiveCanaryBlock {
            approval_id: "operator-approved-canary-001".to_string(),
            no_submit_readiness_report_path: "not-read-before-gate-session-check.json".to_string(),
            max_live_order_count: 1,
            max_notional_per_order: "1.00".to_string(),
            max_no_submit_readiness_report_bytes: 4096,
            readiness_report_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_wait_timeout_seconds: 10,
            reference_quote_probe_actor_id: "no-submit-reference-quote-probe".to_string(),
            reference_quote_probe_log_events: true,
            reference_quote_probe_log_commands: true,
            egress_identity_observed_path: None,
            egress_identity_observed_max_bytes: None,
            approved_egress_identity_sha256: None,
            proof_policy: None,
            operator_evidence: Some(operator_evidence),
        },
    );

    let error = check_bolt_v3_live_canary_gate(&loaded)
        .await
        .expect_err("missing gate session binding must fail closed before report read");
    assert!(
        matches!(
            error,
            BoltV3LiveCanaryGateError::MissingOperatorEvidenceField {
                field: "gate_session_path"
            }
        ),
        "expected missing gate session path rejection, got {error:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn live_canary_gate_rejects_cross_market_gate_session_before_reading_report() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let mut operator_evidence = valid_operator_evidence();
    let gate_session_path = std::path::PathBuf::from(
        operator_evidence
            .gate_session_path
            .as_ref()
            .expect("valid operator evidence should bind gate session"),
    );
    let mut gate_session: serde_json::Value = serde_json::from_slice(
        &std::fs::read(&gate_session_path).expect("gate session should read"),
    )
    .expect("gate session should parse");
    gate_session["selected_market"]["selected_market_key"] = serde_json::json!("c".repeat(64));
    let gate_session_bytes =
        serde_json::to_vec(&gate_session).expect("gate session should serialize");
    std::fs::write(&gate_session_path, &gate_session_bytes).expect("gate session should rewrite");
    operator_evidence.expected_gate_session_sha256 = Some(sha256_hex(&gate_session_bytes));
    let loaded = loaded_with_live_canary(
        loaded,
        LiveCanaryBlock {
            approval_id: "operator-approved-canary-001".to_string(),
            no_submit_readiness_report_path: "not-read-before-gate-session-check.json".to_string(),
            max_live_order_count: 1,
            max_notional_per_order: "1.00".to_string(),
            max_no_submit_readiness_report_bytes: 4096,
            readiness_report_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_wait_timeout_seconds: 10,
            reference_quote_probe_actor_id: "no-submit-reference-quote-probe".to_string(),
            reference_quote_probe_log_events: true,
            reference_quote_probe_log_commands: true,
            egress_identity_observed_path: None,
            egress_identity_observed_max_bytes: None,
            approved_egress_identity_sha256: None,
            proof_policy: None,
            operator_evidence: Some(operator_evidence),
        },
    );

    let error = check_bolt_v3_live_canary_gate(&loaded)
        .await
        .expect_err("cross-market gate session must fail closed before report read");
    assert!(
        matches!(
            error,
            BoltV3LiveCanaryGateError::OperatorGateSessionInvalid { .. }
        ),
        "expected invalid gate session rejection, got {error:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn live_canary_gate_rejects_parent_dir_operator_evidence_path() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let mut operator_evidence = valid_operator_evidence();
    operator_evidence.approval_consumption_path = "../approval-consumption.json".to_string();
    let loaded = loaded_with_live_canary(
        loaded,
        LiveCanaryBlock {
            approval_id: "operator-approved-canary-001".to_string(),
            no_submit_readiness_report_path: "not-read-before-operator-path-check.json".to_string(),
            max_live_order_count: 1,
            max_notional_per_order: "1.00".to_string(),
            max_no_submit_readiness_report_bytes: 4096,
            readiness_report_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_wait_timeout_seconds: 10,
            reference_quote_probe_actor_id: "no-submit-reference-quote-probe".to_string(),
            reference_quote_probe_log_events: true,
            reference_quote_probe_log_commands: true,
            egress_identity_observed_path: None,
            egress_identity_observed_max_bytes: None,
            approved_egress_identity_sha256: None,
            proof_policy: None,
            operator_evidence: Some(operator_evidence),
        },
    );

    let error = check_bolt_v3_live_canary_gate(&loaded)
        .await
        .expect_err("parent directory operator evidence path must fail closed before report read");

    assert!(
        matches!(
            error,
            BoltV3LiveCanaryGateError::InvalidConfiguredPath {
                field: "approval_consumption_path",
                ..
            }
        ),
        "expected invalid operator evidence path rejection, got {error:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn live_canary_gate_rejects_parent_dir_strategy_cancel_path() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let mut operator_evidence = valid_operator_evidence();
    operator_evidence.strategy_cancel_path = Some("../strategy-cancel.json".to_string());
    let loaded = loaded_with_live_canary(
        loaded,
        LiveCanaryBlock {
            approval_id: "operator-approved-canary-001".to_string(),
            no_submit_readiness_report_path: "not-read-before-strategy-cancel-path-check.json"
                .to_string(),
            max_live_order_count: 1,
            max_notional_per_order: "1.00".to_string(),
            max_no_submit_readiness_report_bytes: 4096,
            readiness_report_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_wait_timeout_seconds: 10,
            reference_quote_probe_actor_id: "no-submit-reference-quote-probe".to_string(),
            reference_quote_probe_log_events: true,
            reference_quote_probe_log_commands: true,
            egress_identity_observed_path: None,
            egress_identity_observed_max_bytes: None,
            approved_egress_identity_sha256: None,
            proof_policy: None,
            operator_evidence: Some(operator_evidence),
        },
    );

    let error = check_bolt_v3_live_canary_gate(&loaded)
        .await
        .expect_err("parent directory strategy_cancel_path must fail closed before report read");

    assert!(
        matches!(
            error,
            BoltV3LiveCanaryGateError::InvalidConfiguredPath {
                field: "strategy_cancel_path",
                ..
            }
        ),
        "expected invalid strategy_cancel_path rejection, got {error:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn live_canary_gate_rejects_malformed_operator_evidence_hash_shape() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let mut operator_evidence = valid_operator_evidence();
    operator_evidence.ssm_manifest_sha256 = "not-a-sha256".to_string();
    let loaded = loaded_with_live_canary(
        loaded,
        LiveCanaryBlock {
            approval_id: "operator-approved-canary-001".to_string(),
            no_submit_readiness_report_path: "not-read-before-operator-evidence-hash-check.json"
                .to_string(),
            max_live_order_count: 1,
            max_notional_per_order: "1.00".to_string(),
            max_no_submit_readiness_report_bytes: 4096,
            readiness_report_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_wait_timeout_seconds: 10,
            reference_quote_probe_actor_id: "no-submit-reference-quote-probe".to_string(),
            reference_quote_probe_log_events: true,
            reference_quote_probe_log_commands: true,
            egress_identity_observed_path: None,
            egress_identity_observed_max_bytes: None,
            approved_egress_identity_sha256: None,
            proof_policy: None,
            operator_evidence: Some(operator_evidence),
        },
    );

    let error = check_bolt_v3_live_canary_gate(&loaded)
        .await
        .expect_err("malformed operator evidence hash must fail closed before report read");

    assert!(
        matches!(
            error,
            BoltV3LiveCanaryGateError::InvalidOperatorEvidenceHashShape {
                field: "ssm_manifest_sha256"
            }
        ),
        "expected malformed operator evidence hash rejection, got {error:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn live_canary_gate_rejects_uppercase_operator_evidence_hash_shape() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let mut operator_evidence = valid_operator_evidence();
    operator_evidence.ssm_manifest_sha256 = "A".repeat(64);
    let loaded = loaded_with_live_canary(
        loaded,
        LiveCanaryBlock {
            approval_id: "operator-approved-canary-001".to_string(),
            no_submit_readiness_report_path: "not-read-before-operator-evidence-hash-check.json"
                .to_string(),
            max_live_order_count: 1,
            max_notional_per_order: "1.00".to_string(),
            max_no_submit_readiness_report_bytes: 4096,
            readiness_report_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_wait_timeout_seconds: 10,
            reference_quote_probe_actor_id: "no-submit-reference-quote-probe".to_string(),
            reference_quote_probe_log_events: true,
            reference_quote_probe_log_commands: true,
            egress_identity_observed_path: None,
            egress_identity_observed_max_bytes: None,
            approved_egress_identity_sha256: None,
            proof_policy: None,
            operator_evidence: Some(operator_evidence),
        },
    );

    let error = check_bolt_v3_live_canary_gate(&loaded)
        .await
        .expect_err("uppercase operator evidence hash must fail closed before report read");

    assert!(
        matches!(
            error,
            BoltV3LiveCanaryGateError::InvalidOperatorEvidenceHashShape {
                field: "ssm_manifest_sha256"
            }
        ),
        "expected uppercase operator evidence hash rejection, got {error:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn live_canary_gate_rejects_operator_evidence_file_hash_mismatch() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let report_path = tempdir.path().join("no-submit-readiness.json");
    write_no_submit_report(&report_path, &[]);
    let mut operator_evidence = valid_operator_evidence();
    operator_evidence.ssm_manifest_sha256 = sha256_hex(b"wrong-ssm-manifest");
    let loaded = loaded_with_live_canary(
        loaded,
        LiveCanaryBlock {
            approval_id: "operator-approved-canary-001".to_string(),
            no_submit_readiness_report_path: report_path.to_string_lossy().to_string(),
            max_live_order_count: 1,
            max_notional_per_order: "1.00".to_string(),
            max_no_submit_readiness_report_bytes: 4096,
            readiness_report_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_wait_timeout_seconds: 10,
            reference_quote_probe_actor_id: "no-submit-reference-quote-probe".to_string(),
            reference_quote_probe_log_events: true,
            reference_quote_probe_log_commands: true,
            egress_identity_observed_path: None,
            egress_identity_observed_max_bytes: None,
            approved_egress_identity_sha256: None,
            proof_policy: None,
            operator_evidence: Some(operator_evidence),
        },
    );

    let error = check_bolt_v3_live_canary_gate(&loaded)
        .await
        .expect_err("operator evidence file hash mismatch must fail closed");

    assert!(
        matches!(
            error,
            BoltV3LiveCanaryGateError::OperatorEvidenceHashMismatch {
                field: "ssm_manifest_sha256",
                ..
            }
        ),
        "expected operator evidence hash mismatch, got {error:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn live_canary_gate_rejects_approval_envelope_hash_mismatch() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let report_path = tempdir.path().join("no-submit-readiness.json");
    write_no_submit_report(&report_path, &[]);
    let mut operator_evidence = valid_operator_evidence();
    operator_evidence.approval_envelope_sha256 = sha256_hex(b"wrong-approval-envelope");
    let loaded = loaded_with_live_canary(
        loaded,
        LiveCanaryBlock {
            approval_id: "operator-approved-canary-001".to_string(),
            no_submit_readiness_report_path: report_path.to_string_lossy().to_string(),
            max_live_order_count: 1,
            max_notional_per_order: "1.00".to_string(),
            max_no_submit_readiness_report_bytes: 4096,
            readiness_report_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_wait_timeout_seconds: 10,
            reference_quote_probe_actor_id: "no-submit-reference-quote-probe".to_string(),
            reference_quote_probe_log_events: true,
            reference_quote_probe_log_commands: true,
            egress_identity_observed_path: None,
            egress_identity_observed_max_bytes: None,
            approved_egress_identity_sha256: None,
            proof_policy: None,
            operator_evidence: Some(operator_evidence),
        },
    );

    let error = check_bolt_v3_live_canary_gate(&loaded)
        .await
        .expect_err("approval envelope file hash mismatch must fail closed");

    assert!(
        matches!(
            error,
            BoltV3LiveCanaryGateError::OperatorEvidenceHashMismatch {
                field: "approval_envelope_sha256",
                ..
            }
        ),
        "expected approval envelope hash mismatch, got {error:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn live_canary_gate_rejects_approval_envelope_circular_fields() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let config_bundle_checksum = load_bolt_v3_config(&root_path)
        .expect("fixture v3 config should load")
        .config_bundle_checksum;
    let circular_fields = [
        (
            "approval_envelope_sha256",
            serde_json::json!(valid_operator_evidence().approval_envelope_sha256),
        ),
        (
            "root_toml_sha256",
            serde_json::json!(root_toml_sha256_for_test()),
        ),
        (
            "config_bundle_checksum",
            serde_json::json!(config_bundle_checksum),
        ),
    ];

    for (field, value) in circular_fields {
        let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
        let tempdir = tempfile::tempdir().expect("tempdir should be created");
        let report_path = tempdir.path().join("no-submit-readiness.json");
        write_no_submit_report(&report_path, &[]);
        let mut operator_evidence = valid_operator_evidence();
        let mut envelope = valid_approval_envelope_value(&operator_evidence);
        envelope
            .as_object_mut()
            .expect("approval envelope should be an object")
            .insert(field.to_string(), value);
        bind_approval_envelope_value(&mut operator_evidence, envelope);
        let loaded = loaded_with_live_canary(
            loaded,
            LiveCanaryBlock {
                approval_id: "operator-approved-canary-001".to_string(),
                no_submit_readiness_report_path: report_path.to_string_lossy().to_string(),
                max_live_order_count: 1,
                max_notional_per_order: "1.00".to_string(),
                max_no_submit_readiness_report_bytes: 4096,
                readiness_report_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
                reference_quote_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
                reference_quote_wait_timeout_seconds: 10,
                reference_quote_probe_actor_id: "no-submit-reference-quote-probe".to_string(),
                reference_quote_probe_log_events: true,
                reference_quote_probe_log_commands: true,
                egress_identity_observed_path: None,
                egress_identity_observed_max_bytes: None,
                approved_egress_identity_sha256: None,
                proof_policy: None,
                operator_evidence: Some(operator_evidence),
            },
        );

        let error = match check_bolt_v3_live_canary_gate(&loaded).await {
            Ok(report) => {
                panic!("approval envelope field `{field}` must fail closed, got {report:?}")
            }
            Err(error) => error,
        };
        let rendered = error.to_string();

        assert!(
            rendered.contains("approval envelope") && rendered.contains(field),
            "expected approval envelope schema rejection naming {field}, got {rendered}"
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn live_canary_gate_rejects_approval_envelope_toml_drift_after_hash_match() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let report_path = tempdir.path().join("no-submit-readiness.json");
    write_no_submit_report(&report_path, &[]);
    let mut operator_evidence = valid_operator_evidence();
    let mut envelope = valid_approval_envelope_value(&operator_evidence);
    envelope
        .as_object_mut()
        .expect("approval envelope should be an object")
        .insert(
            "ssm_manifest_sha256".to_string(),
            serde_json::json!(sha256_hex(b"wrong-ssm-manifest")),
        );
    bind_approval_envelope_value(&mut operator_evidence, envelope);
    let loaded = loaded_with_live_canary(
        loaded,
        LiveCanaryBlock {
            approval_id: "operator-approved-canary-001".to_string(),
            no_submit_readiness_report_path: report_path.to_string_lossy().to_string(),
            max_live_order_count: 1,
            max_notional_per_order: "1.00".to_string(),
            max_no_submit_readiness_report_bytes: 4096,
            readiness_report_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_wait_timeout_seconds: 10,
            reference_quote_probe_actor_id: "no-submit-reference-quote-probe".to_string(),
            reference_quote_probe_log_events: true,
            reference_quote_probe_log_commands: true,
            egress_identity_observed_path: None,
            egress_identity_observed_max_bytes: None,
            approved_egress_identity_sha256: None,
            proof_policy: None,
            operator_evidence: Some(operator_evidence),
        },
    );

    let error = check_bolt_v3_live_canary_gate(&loaded)
        .await
        .expect_err("approval envelope TOML drift must fail closed after hash match");
    let rendered = error.to_string();

    assert!(
        rendered.contains("approval envelope") && rendered.contains("ssm_manifest_sha256"),
        "expected approval envelope value-equality rejection naming ssm_manifest_sha256, got {rendered}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn live_canary_gate_accepts_non_circular_approval_envelope_schema() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let report_path = tempdir.path().join("no-submit-readiness.json");
    write_no_submit_report(&report_path, &[]);
    let mut operator_evidence = valid_operator_evidence();
    // Seal the genuine readiness-report hash into the envelope: the report
    // binding is mandatory on the production (proof-disabled) path too.
    seal_no_submit_readiness_report_into_production_evidence(&mut operator_evidence, &report_path);
    let loaded = loaded_with_live_canary(
        loaded,
        LiveCanaryBlock {
            approval_id: "operator-approved-canary-001".to_string(),
            no_submit_readiness_report_path: report_path.to_string_lossy().to_string(),
            max_live_order_count: 1,
            max_notional_per_order: "1.00".to_string(),
            max_no_submit_readiness_report_bytes: 4096,
            readiness_report_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_wait_timeout_seconds: 10,
            reference_quote_probe_actor_id: "no-submit-reference-quote-probe".to_string(),
            reference_quote_probe_log_events: true,
            reference_quote_probe_log_commands: true,
            egress_identity_observed_path: None,
            egress_identity_observed_max_bytes: None,
            approved_egress_identity_sha256: None,
            proof_policy: None,
            operator_evidence: Some(operator_evidence),
        },
    );

    let report = check_bolt_v3_live_canary_gate(&loaded)
        .await
        .expect("non-circular approval envelope should pass");

    assert_eq!(report.approval_id(), "operator-approved-canary-001");
}

#[tokio::test(flavor = "current_thread")]
async fn live_canary_gate_accepts_envelope_bound_gate_session() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let report_path = tempdir.path().join("no-submit-readiness.json");
    write_no_submit_report(&report_path, &[]);
    let mut operator_evidence = valid_operator_evidence();
    // Seal the genuine readiness-report hash into the evidence first: the report
    // binding is mandatory on the production (proof-disabled) path too, so the
    // self-declared TOML hash must be present before the envelope is sealed.
    let report_bytes = std::fs::read(&report_path).expect("readiness report should read");
    operator_evidence.no_submit_readiness_report_sha256 = Some(sha256_hex(&report_bytes));
    // Seal the gate-session hash into the envelope (production behavior) and
    // confirm the gate accepts a gate-session file whose content matches the
    // sealed envelope value.
    let envelope = valid_approval_envelope_value(&operator_evidence);
    assert!(
        envelope
            .get("expected_gate_session_sha256")
            .and_then(serde_json::Value::as_str)
            .is_some(),
        "envelope fixture must seal expected_gate_session_sha256"
    );
    bind_approval_envelope_value(&mut operator_evidence, envelope);
    let loaded = loaded_with_live_canary(
        loaded,
        LiveCanaryBlock {
            approval_id: "operator-approved-canary-001".to_string(),
            no_submit_readiness_report_path: report_path.to_string_lossy().to_string(),
            max_live_order_count: 1,
            max_notional_per_order: "1.00".to_string(),
            max_no_submit_readiness_report_bytes: 4096,
            readiness_report_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_wait_timeout_seconds: 10,
            reference_quote_probe_actor_id: "no-submit-reference-quote-probe".to_string(),
            reference_quote_probe_log_events: true,
            reference_quote_probe_log_commands: true,
            egress_identity_observed_path: None,
            egress_identity_observed_max_bytes: None,
            approved_egress_identity_sha256: None,
            proof_policy: None,
            operator_evidence: Some(operator_evidence),
        },
    );

    let report = check_bolt_v3_live_canary_gate(&loaded)
        .await
        .expect("envelope-bound gate session should pass");

    assert_eq!(report.approval_id(), "operator-approved-canary-001");
}

#[tokio::test(flavor = "current_thread")]
async fn live_canary_gate_rejects_gate_session_swap_after_toml_self_hash_update() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let report_path = tempdir.path().join("no-submit-readiness.json");
    write_no_submit_report(&report_path, &[]);
    let mut operator_evidence = valid_operator_evidence();
    // Seal the original gate-session hash into the envelope, mirroring an
    // operator approval of the original gate session.
    let envelope = valid_approval_envelope_value(&operator_evidence);
    bind_approval_envelope_value(&mut operator_evidence, envelope);

    // Adversary swaps the gate-session file after approval and updates ONLY the
    // self-declared TOML hash to match the swapped file. The envelope still
    // carries the original gate-session hash, so the swap must be rejected.
    let gate_session_path = std::path::PathBuf::from(
        operator_evidence
            .gate_session_path
            .as_ref()
            .expect("valid operator evidence should bind gate session"),
    );
    let mut gate_session: serde_json::Value = serde_json::from_slice(
        &std::fs::read(&gate_session_path).expect("gate session should read"),
    )
    .expect("gate session should parse");
    gate_session["selected_market"]["selected_at_ms"] = serde_json::json!(987_654_321_u64);
    let swapped_gate_session_bytes =
        serde_json::to_vec(&gate_session).expect("gate session should serialize");
    std::fs::write(&gate_session_path, &swapped_gate_session_bytes)
        .expect("gate session should rewrite");
    let swapped_gate_session_sha256 = sha256_hex(&swapped_gate_session_bytes);
    assert_ne!(
        Some(&swapped_gate_session_sha256),
        operator_evidence.expected_gate_session_sha256.as_ref(),
        "swapped gate session must change the file hash"
    );
    // Update only the TOML self-hash so the self-declared check passes; the
    // envelope hash is intentionally left bound to the original gate session.
    operator_evidence.expected_gate_session_sha256 = Some(swapped_gate_session_sha256);
    let loaded = loaded_with_live_canary(
        loaded,
        LiveCanaryBlock {
            approval_id: "operator-approved-canary-001".to_string(),
            no_submit_readiness_report_path: report_path.to_string_lossy().to_string(),
            max_live_order_count: 1,
            max_notional_per_order: "1.00".to_string(),
            max_no_submit_readiness_report_bytes: 4096,
            readiness_report_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_wait_timeout_seconds: 10,
            reference_quote_probe_actor_id: "no-submit-reference-quote-probe".to_string(),
            reference_quote_probe_log_events: true,
            reference_quote_probe_log_commands: true,
            egress_identity_observed_path: None,
            egress_identity_observed_max_bytes: None,
            approved_egress_identity_sha256: None,
            proof_policy: None,
            operator_evidence: Some(operator_evidence),
        },
    );

    let error = check_bolt_v3_live_canary_gate(&loaded)
        .await
        .expect_err("post-approval gate-session swap must fail closed on envelope binding");

    match error {
        BoltV3LiveCanaryGateError::OperatorApprovalEnvelopeMismatch { field, .. } => {
            assert_eq!(
                field, "expected_gate_session_sha256",
                "gate-session swap must be rejected by the envelope binding, got {field}"
            );
        }
        other => panic!("expected approval-envelope gate-session binding rejection, got {other:?}"),
    }
}

/// Proof-policy block matching the shared operator-evidence gate session so the
/// proof-policy path of the live canary gate can be exercised end to end.
fn proof_policy_for_support_gate_session() -> LiveCanaryProofPolicyBlock {
    LiveCanaryProofPolicyBlock {
        enabled: true,
        policy_kind: "least_bad_strategy_candidate".to_string(),
        proof_claim: "proof_only".to_string(),
        executor_strategy_id: "canary-proof-executor-proof".to_string(),
        strategy_instance_id: "configured_updown_main".to_string(),
        execution_client_id: "polymarket_main".to_string(),
        book_type: DataClientReadinessProbeBookType::L2Mbp,
        book_snapshot_interval_millis: 1_000,
        time_in_force: LiveCanaryProofTimeInForce::Fok,
        is_post_only: false,
        is_reduce_only: false,
        is_quote_quantity: false,
        notional_mode: "fixed".to_string(),
        proof_notional: "1.00".to_string(),
        candidate_score_source: "proof_source".to_string(),
        allow_negative_expected_ev: true,
        rotation_observation_enabled: false,
        rotation_min_distinct_markets: 1,
        rotation_max_attempts: 1,
    }
}

/// Configures the shared operator-evidence fixture for the proof-policy path:
/// refreshes the gate-session `created_at_ms` so it passes freshness, writes a
/// canary proof order-intent that binds the gate session, sets the TOML
/// self-declared hashes, and rebuilds the approval envelope sealing both the
/// gate-session and order-intent file hashes (production producer behavior).
fn configure_proof_policy_evidence(
    operator_evidence: &mut LiveCanaryOperatorEvidenceBlock,
    report_path: &std::path::Path,
) {
    // Seal the no-submit readiness-report file hash. The report binding is
    // MANDATORY on the proof-policy path, so the self-declared TOML hash and the
    // envelope must both carry the genuine report-file hash. The caller writes
    // the report before invoking this helper.
    let report_bytes = std::fs::read(report_path).expect("readiness report should read");
    operator_evidence.no_submit_readiness_report_sha256 = Some(sha256_hex(&report_bytes));

    // Refresh the gate-session file so its freshness check passes on the proof
    // path, then re-bind the self-declared TOML hash to the refreshed file.
    let gate_session_path = std::path::PathBuf::from(
        operator_evidence
            .gate_session_path
            .as_ref()
            .expect("valid operator evidence should bind gate session"),
    );
    let mut gate_session: serde_json::Value = serde_json::from_slice(
        &std::fs::read(&gate_session_path).expect("gate session should read"),
    )
    .expect("gate session should parse");
    let now_ms = current_unix_seconds_for_test().saturating_mul(1_000);
    gate_session["created_at_ms"] = serde_json::json!(now_ms);
    let gate_session_bytes =
        serde_json::to_vec(&gate_session).expect("gate session should serialize");
    std::fs::write(&gate_session_path, &gate_session_bytes).expect("gate session should rewrite");
    operator_evidence.expected_gate_session_sha256 = Some(sha256_hex(&gate_session_bytes));

    // Write a canary proof order-intent bound to the gate session and matching
    // the proof policy, then bind its self-declared TOML hash.
    let order_intent_path = gate_session_path
        .parent()
        .expect("gate session path should have a parent")
        .join("canary-proof-order-intent.json");
    let order_intent = serde_json::json!({
        "record_kind": "bolt_v3_canary_proof_order_intent",
        "proof_claim": "proof_only",
        "strategy_instance_id": "configured_updown_main",
        "execution_client_id": "polymarket_main",
        "instrument_id": "configured-condition-UP.POLYMARKET",
        "order_side": "Buy",
        "notional": "1.00",
        "quantity": "2.00",
        "source_refs": ["a".repeat(64)]
    });
    let order_intent_bytes =
        serde_json::to_vec(&order_intent).expect("order intent should serialize");
    std::fs::write(&order_intent_path, &order_intent_bytes)
        .expect("order intent should be written");
    operator_evidence.canary_proof_order_intent_path =
        Some(order_intent_path.to_string_lossy().to_string());
    operator_evidence.canary_proof_order_intent_sha256 = Some(sha256_hex(&order_intent_bytes));

    // Rebuild the envelope so it seals both the refreshed gate-session hash and
    // the order-intent hash, then rebind the envelope file + consumption proof.
    let envelope = valid_approval_envelope_value(operator_evidence);
    bind_approval_envelope_value(operator_evidence, envelope);
}

/// Seals the genuine no-submit readiness-report file hash into the operator
/// evidence for a NON-proof (production strategy run) fixture: sets the
/// self-declared TOML hash and rebuilds the approval envelope so it carries the
/// same hash, then rebinds the envelope file + consumption proof.
///
/// The no-submit report binding is mandatory on EVERY arming path — the
/// proof-disabled production run reads the report and arms `submit_admission`
/// just like the proof canary — so a legitimate production-path fixture must
/// seal the report hash into the envelope exactly as the production producer
/// does. The caller writes the report file before invoking this helper.
fn seal_no_submit_readiness_report_into_production_evidence(
    operator_evidence: &mut LiveCanaryOperatorEvidenceBlock,
    report_path: &std::path::Path,
) {
    let report_bytes = std::fs::read(report_path).expect("readiness report should read");
    operator_evidence.no_submit_readiness_report_sha256 = Some(sha256_hex(&report_bytes));
    let envelope = valid_approval_envelope_value(operator_evidence);
    bind_approval_envelope_value(operator_evidence, envelope);
}

#[tokio::test(flavor = "current_thread")]
async fn live_canary_gate_accepts_envelope_bound_canary_proof_order_intent() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let report_path = tempdir.path().join("no-submit-readiness.json");
    write_no_submit_report(&report_path, &[]);
    let mut operator_evidence = valid_operator_evidence();
    configure_proof_policy_evidence(&mut operator_evidence, &report_path);
    let loaded = loaded_with_live_canary(
        loaded,
        LiveCanaryBlock {
            approval_id: "operator-approved-canary-001".to_string(),
            no_submit_readiness_report_path: report_path.to_string_lossy().to_string(),
            max_live_order_count: 1,
            max_notional_per_order: "1.00".to_string(),
            max_no_submit_readiness_report_bytes: 4096,
            readiness_report_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_wait_timeout_seconds: 10,
            reference_quote_probe_actor_id: "no-submit-reference-quote-probe".to_string(),
            reference_quote_probe_log_events: true,
            reference_quote_probe_log_commands: true,
            egress_identity_observed_path: None,
            egress_identity_observed_max_bytes: None,
            approved_egress_identity_sha256: None,
            proof_policy: Some(proof_policy_for_support_gate_session()),
            operator_evidence: Some(operator_evidence),
        },
    );

    let report = check_bolt_v3_live_canary_gate(&loaded)
        .await
        .expect("envelope-bound canary proof order intent should pass on the proof path");

    assert_eq!(report.approval_id(), "operator-approved-canary-001");
}

#[tokio::test(flavor = "current_thread")]
async fn live_canary_gate_rejects_canary_proof_order_intent_swap_after_toml_self_hash_update() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let report_path = tempdir.path().join("no-submit-readiness.json");
    write_no_submit_report(&report_path, &[]);
    let mut operator_evidence = valid_operator_evidence();
    configure_proof_policy_evidence(&mut operator_evidence, &report_path);

    // Adversary swaps the canary proof order-intent file after approval and
    // updates ONLY the self-declared TOML hash. The envelope still seals the
    // original order-intent hash, so the swap must be rejected.
    let order_intent_path = std::path::PathBuf::from(
        operator_evidence
            .canary_proof_order_intent_path
            .as_ref()
            .expect("proof policy evidence should bind order intent"),
    );
    let mut order_intent: serde_json::Value = serde_json::from_slice(
        &std::fs::read(&order_intent_path).expect("order intent should read"),
    )
    .expect("order intent should parse");
    // Redirect the order to the other side of the market while keeping it a
    // structurally valid, gate-session-bound proof order intent.
    order_intent["instrument_id"] = serde_json::json!("configured-condition-DOWN.POLYMARKET");
    let swapped_order_intent_bytes =
        serde_json::to_vec(&order_intent).expect("order intent should serialize");
    std::fs::write(&order_intent_path, &swapped_order_intent_bytes)
        .expect("order intent should rewrite");
    let swapped_order_intent_sha256 = sha256_hex(&swapped_order_intent_bytes);
    assert_ne!(
        Some(&swapped_order_intent_sha256),
        operator_evidence.canary_proof_order_intent_sha256.as_ref(),
        "swapped order intent must change the file hash"
    );
    operator_evidence.canary_proof_order_intent_sha256 = Some(swapped_order_intent_sha256);
    let loaded = loaded_with_live_canary(
        loaded,
        LiveCanaryBlock {
            approval_id: "operator-approved-canary-001".to_string(),
            no_submit_readiness_report_path: report_path.to_string_lossy().to_string(),
            max_live_order_count: 1,
            max_notional_per_order: "1.00".to_string(),
            max_no_submit_readiness_report_bytes: 4096,
            readiness_report_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_wait_timeout_seconds: 10,
            reference_quote_probe_actor_id: "no-submit-reference-quote-probe".to_string(),
            reference_quote_probe_log_events: true,
            reference_quote_probe_log_commands: true,
            egress_identity_observed_path: None,
            egress_identity_observed_max_bytes: None,
            approved_egress_identity_sha256: None,
            proof_policy: Some(proof_policy_for_support_gate_session()),
            operator_evidence: Some(operator_evidence),
        },
    );

    let error = check_bolt_v3_live_canary_gate(&loaded)
        .await
        .expect_err("post-approval order-intent swap must fail closed on envelope binding");

    match error {
        BoltV3LiveCanaryGateError::OperatorApprovalEnvelopeMismatch { field, .. } => {
            assert_eq!(
                field, "canary_proof_order_intent_sha256",
                "order-intent swap must be rejected by the envelope binding, got {field}"
            );
        }
        other => {
            panic!("expected approval-envelope order-intent binding rejection, got {other:?}")
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn live_canary_gate_rejects_missing_envelope_order_intent_binding_on_proof_path() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let report_path = tempdir.path().join("no-submit-readiness.json");
    write_no_submit_report(&report_path, &[]);
    let mut operator_evidence = valid_operator_evidence();
    configure_proof_policy_evidence(&mut operator_evidence, &report_path);

    // Strip the order-intent binding from the envelope (a legacy/forged
    // envelope) while keeping the self-declared TOML hash. The proof path must
    // fail closed because the envelope binding is mandatory there.
    let mut envelope = valid_approval_envelope_value(&operator_evidence);
    envelope
        .as_object_mut()
        .expect("approval envelope should be an object")
        .remove("canary_proof_order_intent_sha256");
    bind_approval_envelope_value(&mut operator_evidence, envelope);
    let loaded = loaded_with_live_canary(
        loaded,
        LiveCanaryBlock {
            approval_id: "operator-approved-canary-001".to_string(),
            no_submit_readiness_report_path: report_path.to_string_lossy().to_string(),
            max_live_order_count: 1,
            max_notional_per_order: "1.00".to_string(),
            max_no_submit_readiness_report_bytes: 4096,
            readiness_report_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_wait_timeout_seconds: 10,
            reference_quote_probe_actor_id: "no-submit-reference-quote-probe".to_string(),
            reference_quote_probe_log_events: true,
            reference_quote_probe_log_commands: true,
            egress_identity_observed_path: None,
            egress_identity_observed_max_bytes: None,
            approved_egress_identity_sha256: None,
            proof_policy: Some(proof_policy_for_support_gate_session()),
            operator_evidence: Some(operator_evidence),
        },
    );

    let error = check_bolt_v3_live_canary_gate(&loaded)
        .await
        .expect_err("missing envelope order-intent binding must fail closed on the proof path");

    match error {
        BoltV3LiveCanaryGateError::MissingOperatorEvidenceField { field } => {
            assert_eq!(
                field, "canary_proof_order_intent_sha256",
                "missing envelope order-intent binding must fail closed, got {field}"
            );
        }
        other => panic!("expected missing envelope order-intent binding rejection, got {other:?}"),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn live_canary_gate_accepts_envelope_bound_no_submit_readiness_report() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let report_path = tempdir.path().join("no-submit-readiness.json");
    write_no_submit_report(&report_path, &[]);
    let mut operator_evidence = valid_operator_evidence();
    // Seals the genuine readiness-report file hash into the envelope (and the
    // self-declared TOML hash) alongside the gate-session and order-intent
    // bindings — production producer behavior.
    configure_proof_policy_evidence(&mut operator_evidence, &report_path);
    let loaded = loaded_with_live_canary(
        loaded,
        LiveCanaryBlock {
            approval_id: "operator-approved-canary-001".to_string(),
            no_submit_readiness_report_path: report_path.to_string_lossy().to_string(),
            max_live_order_count: 1,
            max_notional_per_order: "1.00".to_string(),
            max_no_submit_readiness_report_bytes: 4096,
            readiness_report_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_wait_timeout_seconds: 10,
            reference_quote_probe_actor_id: "no-submit-reference-quote-probe".to_string(),
            reference_quote_probe_log_events: true,
            reference_quote_probe_log_commands: true,
            egress_identity_observed_path: None,
            egress_identity_observed_max_bytes: None,
            approved_egress_identity_sha256: None,
            proof_policy: Some(proof_policy_for_support_gate_session()),
            operator_evidence: Some(operator_evidence),
        },
    );

    let report = check_bolt_v3_live_canary_gate(&loaded)
        .await
        .expect("envelope-bound no-submit readiness report should pass on the proof path");

    assert_eq!(report.approval_id(), "operator-approved-canary-001");
}

#[tokio::test(flavor = "current_thread")]
async fn live_canary_gate_rejects_no_submit_readiness_report_swap_after_toml_self_hash_update() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let report_path = tempdir.path().join("no-submit-readiness.json");
    write_no_submit_report(&report_path, &[]);
    let mut operator_evidence = valid_operator_evidence();
    // Seal the ORIGINAL readiness-report hash into the envelope, mirroring an
    // operator approval of the probe-produced report.
    configure_proof_policy_evidence(&mut operator_evidence, &report_path);

    // Adversary replaces the readiness report at the configured path with a
    // hand-written, still-all-satisfied report whose linkage fields are forged
    // to pass the content check, then updates ONLY the self-declared TOML hash
    // to match the forged file. The envelope still seals the original report
    // hash, so the swap must be rejected.
    let original_report_sha256 = operator_evidence
        .no_submit_readiness_report_sha256
        .clone()
        .expect("proof policy evidence should bind readiness report");
    // Forge a still-fresh, still-all-satisfied report with a different
    // generated_at so its content (hence sha256) differs from the approved one.
    write_no_submit_report_at(
        &report_path,
        &[],
        current_unix_seconds_for_test().saturating_sub(1),
    );
    let forged_report_bytes = std::fs::read(&report_path).expect("forged report should read");
    let forged_report_sha256 = sha256_hex(&forged_report_bytes);
    assert_ne!(
        forged_report_sha256, original_report_sha256,
        "forged readiness report must change the file hash"
    );
    // Update only the TOML self-hash so the self-declared check passes; the
    // envelope hash is intentionally left bound to the original report.
    operator_evidence.no_submit_readiness_report_sha256 = Some(forged_report_sha256);
    let loaded = loaded_with_live_canary(
        loaded,
        LiveCanaryBlock {
            approval_id: "operator-approved-canary-001".to_string(),
            no_submit_readiness_report_path: report_path.to_string_lossy().to_string(),
            max_live_order_count: 1,
            max_notional_per_order: "1.00".to_string(),
            max_no_submit_readiness_report_bytes: 4096,
            readiness_report_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_wait_timeout_seconds: 10,
            reference_quote_probe_actor_id: "no-submit-reference-quote-probe".to_string(),
            reference_quote_probe_log_events: true,
            reference_quote_probe_log_commands: true,
            egress_identity_observed_path: None,
            egress_identity_observed_max_bytes: None,
            approved_egress_identity_sha256: None,
            proof_policy: Some(proof_policy_for_support_gate_session()),
            operator_evidence: Some(operator_evidence),
        },
    );

    let error = check_bolt_v3_live_canary_gate(&loaded)
        .await
        .expect_err("post-approval readiness-report swap must fail closed on envelope binding");

    match error {
        BoltV3LiveCanaryGateError::OperatorApprovalEnvelopeMismatch { field, .. } => {
            assert_eq!(
                field, "no_submit_readiness_report_sha256",
                "readiness-report swap must be rejected by the envelope binding, got {field}"
            );
        }
        other => {
            panic!("expected approval-envelope readiness-report binding rejection, got {other:?}")
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn live_canary_gate_rejects_missing_envelope_no_submit_readiness_report_binding_on_proof_path()
 {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let report_path = tempdir.path().join("no-submit-readiness.json");
    write_no_submit_report(&report_path, &[]);
    let mut operator_evidence = valid_operator_evidence();
    configure_proof_policy_evidence(&mut operator_evidence, &report_path);

    // Strip the readiness-report binding from the envelope (a legacy/forged
    // envelope) while keeping the self-declared TOML hash. The proof path must
    // fail closed because the envelope binding is mandatory there.
    let mut envelope = valid_approval_envelope_value(&operator_evidence);
    envelope
        .as_object_mut()
        .expect("approval envelope should be an object")
        .remove("no_submit_readiness_report_sha256");
    bind_approval_envelope_value(&mut operator_evidence, envelope);
    let loaded = loaded_with_live_canary(
        loaded,
        LiveCanaryBlock {
            approval_id: "operator-approved-canary-001".to_string(),
            no_submit_readiness_report_path: report_path.to_string_lossy().to_string(),
            max_live_order_count: 1,
            max_notional_per_order: "1.00".to_string(),
            max_no_submit_readiness_report_bytes: 4096,
            readiness_report_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_wait_timeout_seconds: 10,
            reference_quote_probe_actor_id: "no-submit-reference-quote-probe".to_string(),
            reference_quote_probe_log_events: true,
            reference_quote_probe_log_commands: true,
            egress_identity_observed_path: None,
            egress_identity_observed_max_bytes: None,
            approved_egress_identity_sha256: None,
            proof_policy: Some(proof_policy_for_support_gate_session()),
            operator_evidence: Some(operator_evidence),
        },
    );

    let error = check_bolt_v3_live_canary_gate(&loaded)
        .await
        .expect_err("missing envelope readiness-report binding must fail closed on the proof path");

    match error {
        BoltV3LiveCanaryGateError::MissingOperatorEvidenceField { field } => {
            assert_eq!(
                field, "no_submit_readiness_report_sha256",
                "missing envelope readiness-report binding must fail closed, got {field}"
            );
        }
        other => {
            panic!("expected missing envelope readiness-report binding rejection, got {other:?}")
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn live_canary_gate_rejects_no_submit_readiness_report_swap_after_toml_self_hash_update_on_production_path()
 {
    // P4 forge attack on the PRODUCTION (proof-disabled) path: the no-submit
    // readiness report is read and consumed to arm submit_admission on the
    // proof-disabled production strategy run too, so its envelope binding is
    // mandatory there. An adversary who replaces the report at the configured
    // path with a hand-written all-satisfied report and updates ONLY the
    // self-declared TOML hash must still be rejected by the operator-sealed
    // envelope. Pre-fix this swap armed real orders; post-fix it fails closed.
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let report_path = tempdir.path().join("no-submit-readiness.json");
    write_no_submit_report(&report_path, &[]);
    let mut operator_evidence = valid_operator_evidence();
    // Seal the ORIGINAL readiness-report hash into the envelope (and TOML),
    // mirroring an operator approval of the probe-produced report, with proof
    // policy DISABLED.
    seal_no_submit_readiness_report_into_production_evidence(&mut operator_evidence, &report_path);
    let original_report_sha256 = operator_evidence
        .no_submit_readiness_report_sha256
        .clone()
        .expect("production evidence should bind readiness report");

    // Forge a still-fresh, still-all-satisfied report with a different
    // generated_at so its content (hence sha256) differs from the approved one.
    write_no_submit_report_at(
        &report_path,
        &[],
        current_unix_seconds_for_test().saturating_sub(1),
    );
    let forged_report_bytes = std::fs::read(&report_path).expect("forged report should read");
    let forged_report_sha256 = sha256_hex(&forged_report_bytes);
    assert_ne!(
        forged_report_sha256, original_report_sha256,
        "forged readiness report must change the file hash"
    );
    // Update only the TOML self-hash so the self-declared check passes; the
    // envelope hash is intentionally left bound to the original report.
    operator_evidence.no_submit_readiness_report_sha256 = Some(forged_report_sha256);
    let loaded = loaded_with_live_canary(
        loaded,
        LiveCanaryBlock {
            approval_id: "operator-approved-canary-001".to_string(),
            no_submit_readiness_report_path: report_path.to_string_lossy().to_string(),
            max_live_order_count: 1,
            max_notional_per_order: "1.00".to_string(),
            max_no_submit_readiness_report_bytes: 4096,
            readiness_report_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_wait_timeout_seconds: 10,
            reference_quote_probe_actor_id: "no-submit-reference-quote-probe".to_string(),
            reference_quote_probe_log_events: true,
            reference_quote_probe_log_commands: true,
            egress_identity_observed_path: None,
            egress_identity_observed_max_bytes: None,
            approved_egress_identity_sha256: None,
            proof_policy: None,
            operator_evidence: Some(operator_evidence),
        },
    );

    let error = check_bolt_v3_live_canary_gate(&loaded)
        .await
        .expect_err("post-approval readiness-report swap must fail closed on the production path");

    match error {
        BoltV3LiveCanaryGateError::OperatorApprovalEnvelopeMismatch { field, .. } => {
            assert_eq!(
                field, "no_submit_readiness_report_sha256",
                "production-path readiness-report swap must be rejected by the envelope binding, got {field}"
            );
        }
        other => {
            panic!(
                "expected production-path approval-envelope readiness-report binding rejection, got {other:?}"
            )
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn live_canary_gate_rejects_missing_envelope_no_submit_readiness_report_binding_on_production_path()
 {
    // On the production (proof-disabled) path the report binding is mandatory
    // too, so an envelope that OMITS no_submit_readiness_report_sha256 (a
    // legacy/forged envelope) must fail closed even though the self-declared
    // TOML hash is present.
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let report_path = tempdir.path().join("no-submit-readiness.json");
    write_no_submit_report(&report_path, &[]);
    let mut operator_evidence = valid_operator_evidence();
    // Seal the genuine report hash into the TOML evidence, then build an envelope
    // that omits the report binding while keeping the self-declared TOML hash.
    let report_bytes = std::fs::read(&report_path).expect("readiness report should read");
    operator_evidence.no_submit_readiness_report_sha256 = Some(sha256_hex(&report_bytes));
    let mut envelope = valid_approval_envelope_value(&operator_evidence);
    envelope
        .as_object_mut()
        .expect("approval envelope should be an object")
        .remove("no_submit_readiness_report_sha256");
    bind_approval_envelope_value(&mut operator_evidence, envelope);
    let loaded = loaded_with_live_canary(
        loaded,
        LiveCanaryBlock {
            approval_id: "operator-approved-canary-001".to_string(),
            no_submit_readiness_report_path: report_path.to_string_lossy().to_string(),
            max_live_order_count: 1,
            max_notional_per_order: "1.00".to_string(),
            max_no_submit_readiness_report_bytes: 4096,
            readiness_report_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_wait_timeout_seconds: 10,
            reference_quote_probe_actor_id: "no-submit-reference-quote-probe".to_string(),
            reference_quote_probe_log_events: true,
            reference_quote_probe_log_commands: true,
            egress_identity_observed_path: None,
            egress_identity_observed_max_bytes: None,
            approved_egress_identity_sha256: None,
            proof_policy: None,
            operator_evidence: Some(operator_evidence),
        },
    );

    let error = check_bolt_v3_live_canary_gate(&loaded).await.expect_err(
        "missing envelope readiness-report binding must fail closed on the production path",
    );

    match error {
        BoltV3LiveCanaryGateError::MissingOperatorEvidenceField { field } => {
            assert_eq!(
                field, "no_submit_readiness_report_sha256",
                "missing envelope readiness-report binding must fail closed on production, got {field}"
            );
        }
        other => {
            panic!(
                "expected missing envelope readiness-report binding rejection on production, got {other:?}"
            )
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn live_canary_gate_accepts_envelope_bound_no_submit_readiness_report_on_production_path() {
    // A legitimate production (proof-disabled) run with the genuine report hash
    // sealed into both the self-declared TOML evidence and the operator-approval
    // envelope must still pass the gate.
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let report_path = tempdir.path().join("no-submit-readiness.json");
    write_no_submit_report(&report_path, &[]);
    let mut operator_evidence = valid_operator_evidence();
    // Seal the genuine readiness-report file hash into the envelope (and the
    // self-declared TOML hash) — production producer behavior, proof DISABLED.
    seal_no_submit_readiness_report_into_production_evidence(&mut operator_evidence, &report_path);
    let loaded = loaded_with_live_canary(
        loaded,
        LiveCanaryBlock {
            approval_id: "operator-approved-canary-001".to_string(),
            no_submit_readiness_report_path: report_path.to_string_lossy().to_string(),
            max_live_order_count: 1,
            max_notional_per_order: "1.00".to_string(),
            max_no_submit_readiness_report_bytes: 4096,
            readiness_report_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_wait_timeout_seconds: 10,
            reference_quote_probe_actor_id: "no-submit-reference-quote-probe".to_string(),
            reference_quote_probe_log_events: true,
            reference_quote_probe_log_commands: true,
            egress_identity_observed_path: None,
            egress_identity_observed_max_bytes: None,
            approved_egress_identity_sha256: None,
            proof_policy: None,
            operator_evidence: Some(operator_evidence),
        },
    );

    let report = check_bolt_v3_live_canary_gate(&loaded)
        .await
        .expect("envelope-bound no-submit readiness report should pass on the production path");

    assert_eq!(report.approval_id(), "operator-approved-canary-001");
}

#[tokio::test(flavor = "current_thread")]
async fn live_canary_gate_rejects_approval_consumption_hash_mismatch() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let report_path = tempdir.path().join("no-submit-readiness.json");
    write_no_submit_report(&report_path, &[]);
    let operator_evidence = valid_operator_evidence();
    write_approval_consumption_proof_with_override(
        &operator_evidence,
        "canary_evidence_path_hash",
        serde_json::json!(sha256_hex(b"wrong-canary-evidence-path")),
    );
    let loaded = loaded_with_live_canary(
        loaded,
        LiveCanaryBlock {
            approval_id: "operator-approved-canary-001".to_string(),
            no_submit_readiness_report_path: report_path.to_string_lossy().to_string(),
            max_live_order_count: 1,
            max_notional_per_order: "1.00".to_string(),
            max_no_submit_readiness_report_bytes: 4096,
            readiness_report_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_wait_timeout_seconds: 10,
            reference_quote_probe_actor_id: "no-submit-reference-quote-probe".to_string(),
            reference_quote_probe_log_events: true,
            reference_quote_probe_log_commands: true,
            egress_identity_observed_path: None,
            egress_identity_observed_max_bytes: None,
            approved_egress_identity_sha256: None,
            proof_policy: None,
            operator_evidence: Some(operator_evidence),
        },
    );

    let error = check_bolt_v3_live_canary_gate(&loaded)
        .await
        .expect_err("approval consumption proof mismatch must fail closed");

    assert!(
        matches!(
            error,
            BoltV3LiveCanaryGateError::OperatorApprovalConsumptionMismatch {
                field: "canary_evidence_path_hash",
                ..
            }
        ),
        "expected approval consumption mismatch, got {error:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn live_canary_gate_rejects_approval_consumption_strategy_cancel_path_hash_mismatch() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let report_path = tempdir.path().join("no-submit-readiness.json");
    write_no_submit_report(&report_path, &[]);
    let operator_evidence = valid_operator_evidence();
    assert!(
        operator_evidence.strategy_cancel_path.is_some(),
        "fixture must configure strategy_cancel_path to prove the optional path binding"
    );
    write_approval_consumption_proof_with_override(
        &operator_evidence,
        "strategy_cancel_path_hash",
        serde_json::json!(sha256_hex(b"wrong-strategy-cancel-path")),
    );
    let loaded = loaded_with_live_canary(
        loaded,
        LiveCanaryBlock {
            approval_id: "operator-approved-canary-001".to_string(),
            no_submit_readiness_report_path: report_path.to_string_lossy().to_string(),
            max_live_order_count: 1,
            max_notional_per_order: "1.00".to_string(),
            max_no_submit_readiness_report_bytes: 4096,
            readiness_report_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_wait_timeout_seconds: 10,
            reference_quote_probe_actor_id: "no-submit-reference-quote-probe".to_string(),
            reference_quote_probe_log_events: true,
            reference_quote_probe_log_commands: true,
            egress_identity_observed_path: None,
            egress_identity_observed_max_bytes: None,
            approved_egress_identity_sha256: None,
            proof_policy: None,
            operator_evidence: Some(operator_evidence),
        },
    );

    let error = check_bolt_v3_live_canary_gate(&loaded)
        .await
        .expect_err("strategy_cancel_path_hash mismatch must fail closed");

    assert!(
        matches!(
            error,
            BoltV3LiveCanaryGateError::OperatorApprovalConsumptionMismatch {
                field: "strategy_cancel_path_hash",
                ..
            }
        ),
        "expected strategy_cancel_path_hash mismatch, got {error:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn live_canary_gate_rejects_approval_consumption_missing_head_sha() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let report_path = tempdir.path().join("no-submit-readiness.json");
    write_no_submit_report(&report_path, &[]);
    let operator_evidence = valid_operator_evidence();
    write_approval_consumption_proof_without_field(&operator_evidence, "head_sha");
    let loaded = loaded_with_live_canary(
        loaded,
        LiveCanaryBlock {
            approval_id: "operator-approved-canary-001".to_string(),
            no_submit_readiness_report_path: report_path.to_string_lossy().to_string(),
            max_live_order_count: 1,
            max_notional_per_order: "1.00".to_string(),
            max_no_submit_readiness_report_bytes: 4096,
            readiness_report_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_wait_timeout_seconds: 10,
            reference_quote_probe_actor_id: "no-submit-reference-quote-probe".to_string(),
            reference_quote_probe_log_events: true,
            reference_quote_probe_log_commands: true,
            egress_identity_observed_path: None,
            egress_identity_observed_max_bytes: None,
            approved_egress_identity_sha256: None,
            proof_policy: None,
            operator_evidence: Some(operator_evidence),
        },
    );

    let error = check_bolt_v3_live_canary_gate(&loaded)
        .await
        .expect_err("approval consumption proof missing head_sha must fail closed");

    assert!(
        matches!(
            error,
            BoltV3LiveCanaryGateError::OperatorApprovalConsumptionMalformed { .. }
        ) && error.to_string().contains("head_sha"),
        "expected missing head_sha rejection, got {error:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn live_canary_gate_rejects_approval_consumption_head_sha_mismatch() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let report_path = tempdir.path().join("no-submit-readiness.json");
    write_no_submit_report(&report_path, &[]);
    let operator_evidence = valid_operator_evidence();
    write_approval_consumption_proof_with_override(
        &operator_evidence,
        "head_sha",
        serde_json::json!("fedcba9876543210fedcba9876543210fedcba98"),
    );
    let loaded = loaded_with_live_canary(
        loaded,
        LiveCanaryBlock {
            approval_id: "operator-approved-canary-001".to_string(),
            no_submit_readiness_report_path: report_path.to_string_lossy().to_string(),
            max_live_order_count: 1,
            max_notional_per_order: "1.00".to_string(),
            max_no_submit_readiness_report_bytes: 4096,
            readiness_report_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_wait_timeout_seconds: 10,
            reference_quote_probe_actor_id: "no-submit-reference-quote-probe".to_string(),
            reference_quote_probe_log_events: true,
            reference_quote_probe_log_commands: true,
            egress_identity_observed_path: None,
            egress_identity_observed_max_bytes: None,
            approved_egress_identity_sha256: None,
            proof_policy: None,
            operator_evidence: Some(operator_evidence),
        },
    );

    let error = check_bolt_v3_live_canary_gate(&loaded)
        .await
        .expect_err("approval consumption proof head_sha mismatch must fail closed");

    assert!(
        matches!(
            error,
            BoltV3LiveCanaryGateError::OperatorApprovalConsumptionMismatch {
                field: "head_sha",
                ..
            }
        ),
        "expected head_sha mismatch, got {error:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn live_canary_gate_rejects_stale_self_consistent_head_sha() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let report_path = tempdir.path().join("no-submit-readiness.json");
    write_no_submit_report(&report_path, &[]);
    let mut operator_evidence = valid_operator_evidence();
    operator_evidence.head_sha = "fedcba9876543210fedcba9876543210fedcba98".to_string();
    write_valid_approval_consumption_proof(&operator_evidence);
    let loaded = loaded_with_live_canary(
        loaded,
        LiveCanaryBlock {
            approval_id: "operator-approved-canary-001".to_string(),
            no_submit_readiness_report_path: report_path.to_string_lossy().to_string(),
            max_live_order_count: 1,
            max_notional_per_order: "1.00".to_string(),
            max_no_submit_readiness_report_bytes: 4096,
            readiness_report_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_wait_timeout_seconds: 10,
            reference_quote_probe_actor_id: "no-submit-reference-quote-probe".to_string(),
            reference_quote_probe_log_events: true,
            reference_quote_probe_log_commands: true,
            egress_identity_observed_path: None,
            egress_identity_observed_max_bytes: None,
            approved_egress_identity_sha256: None,
            proof_policy: None,
            operator_evidence: Some(operator_evidence),
        },
    );

    let error = check_bolt_v3_live_canary_gate(&loaded)
        .await
        .expect_err("stale self-consistent head_sha must fail closed");

    assert!(
        matches!(
            error,
            BoltV3LiveCanaryGateError::OperatorEvidenceHeadShaMismatch {
                ref actual,
                ..
            } if actual == "fedcba9876543210fedcba9876543210fedcba98"
        ),
        "expected stale head_sha rejection, got {error:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn live_canary_gate_rejects_malformed_operator_evidence_head_sha() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let report_path = tempdir.path().join("no-submit-readiness.json");
    write_no_submit_report(&report_path, &[]);
    let mut operator_evidence = valid_operator_evidence();
    operator_evidence.head_sha = "ABCDEF".to_string();
    write_valid_approval_consumption_proof(&operator_evidence);
    let loaded = loaded_with_live_canary(
        loaded,
        LiveCanaryBlock {
            approval_id: "operator-approved-canary-001".to_string(),
            no_submit_readiness_report_path: report_path.to_string_lossy().to_string(),
            max_live_order_count: 1,
            max_notional_per_order: "1.00".to_string(),
            max_no_submit_readiness_report_bytes: 4096,
            readiness_report_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_wait_timeout_seconds: 10,
            reference_quote_probe_actor_id: "no-submit-reference-quote-probe".to_string(),
            reference_quote_probe_log_events: true,
            reference_quote_probe_log_commands: true,
            egress_identity_observed_path: None,
            egress_identity_observed_max_bytes: None,
            approved_egress_identity_sha256: None,
            proof_policy: None,
            operator_evidence: Some(operator_evidence),
        },
    );

    let error = check_bolt_v3_live_canary_gate(&loaded)
        .await
        .expect_err("malformed operator evidence head_sha must fail closed");

    assert!(
        matches!(
            error,
            BoltV3LiveCanaryGateError::InvalidOperatorEvidenceHeadShaShape { field: "head_sha" }
        ),
        "expected malformed head_sha rejection, got {error:?}"
    );
}

#[test]
fn live_canary_gate_hashes_root_toml_without_blocking_async_gate_thread() {
    let source = support::repo_text("src/bolt_v3_live_canary_gate.rs");

    assert!(
        source.contains("async fn root_toml_sha256"),
        "root_toml_sha256 must be async because it is called from the async gate"
    );
    assert!(
        source.contains("bounded_config_read::read_to_string_async(root_path)")
            && source.contains(".await"),
        "root_toml_sha256 must use async bounded config I/O"
    );
    assert!(
        !source.contains("bounded_config_read::read_to_string(root_path)"),
        "async live canary gate must not call sync bounded_config_read::read_to_string"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn live_canary_gate_rejects_approval_consumption_root_toml_sha256_mismatch() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let report_path = tempdir.path().join("no-submit-readiness.json");
    write_no_submit_report(&report_path, &[]);
    let operator_evidence = valid_operator_evidence();
    write_approval_consumption_proof_with_override(
        &operator_evidence,
        "root_toml_sha256",
        serde_json::json!(sha256_hex(b"wrong-root-toml")),
    );
    let loaded = loaded_with_live_canary(
        loaded,
        LiveCanaryBlock {
            approval_id: "operator-approved-canary-001".to_string(),
            no_submit_readiness_report_path: report_path.to_string_lossy().to_string(),
            max_live_order_count: 1,
            max_notional_per_order: "1.00".to_string(),
            max_no_submit_readiness_report_bytes: 4096,
            readiness_report_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_wait_timeout_seconds: 10,
            reference_quote_probe_actor_id: "no-submit-reference-quote-probe".to_string(),
            reference_quote_probe_log_events: true,
            reference_quote_probe_log_commands: true,
            egress_identity_observed_path: None,
            egress_identity_observed_max_bytes: None,
            approved_egress_identity_sha256: None,
            proof_policy: None,
            operator_evidence: Some(operator_evidence),
        },
    );

    let error = check_bolt_v3_live_canary_gate(&loaded)
        .await
        .expect_err("approval consumption proof root_toml_sha256 mismatch must fail closed");

    assert!(
        matches!(
            error,
            BoltV3LiveCanaryGateError::OperatorApprovalConsumptionMismatch {
                field: "root_toml_sha256",
                ..
            }
        ),
        "expected root_toml_sha256 mismatch, got {error:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn live_canary_gate_rejects_stale_approval_consumption_beyond_configured_max_age() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let report_path = tempdir.path().join("no-submit-readiness.json");
    write_no_submit_report(&report_path, &[]);
    let now = current_unix_seconds_for_test() as i64;
    let mut operator_evidence = valid_operator_evidence_for_window(now - 600, now + 600);
    operator_evidence.approval_consumption_max_age_seconds = 60;
    write_approval_consumption_proof_with_override(
        &operator_evidence,
        "consumed_unix_secs",
        serde_json::json!(now - 120),
    );
    let loaded = loaded_with_live_canary(
        loaded,
        LiveCanaryBlock {
            approval_id: "operator-approved-canary-001".to_string(),
            no_submit_readiness_report_path: report_path.to_string_lossy().to_string(),
            max_live_order_count: 1,
            max_notional_per_order: "1.00".to_string(),
            max_no_submit_readiness_report_bytes: 4096,
            readiness_report_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_wait_timeout_seconds: 10,
            reference_quote_probe_actor_id: "no-submit-reference-quote-probe".to_string(),
            reference_quote_probe_log_events: true,
            reference_quote_probe_log_commands: true,
            egress_identity_observed_path: None,
            egress_identity_observed_max_bytes: None,
            approved_egress_identity_sha256: None,
            proof_policy: None,
            operator_evidence: Some(operator_evidence),
        },
    );

    let error = check_bolt_v3_live_canary_gate(&loaded)
        .await
        .expect_err("stale approval consumption proof must fail closed");

    assert!(
        matches!(
            error,
            BoltV3LiveCanaryGateError::OperatorApprovalConsumptionStale { .. }
        ),
        "expected stale approval consumption rejection, got {error:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn live_canary_gate_rejects_zero_operator_evidence_file_byte_cap() {
    let mut operator_evidence = valid_operator_evidence();
    operator_evidence.max_operator_evidence_file_bytes = 0;

    let error =
        check_operator_evidence_rejection(operator_evidence, "max_operator_evidence_file_bytes")
            .await;

    assert!(
        matches!(
            error,
            BoltV3LiveCanaryGateError::InvalidOperatorEvidenceSizeLimit { value: 0 }
        ),
        "expected operator evidence byte-cap rejection, got {error:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn live_canary_gate_rejects_zero_approval_consumption_max_age() {
    let mut operator_evidence = valid_operator_evidence();
    operator_evidence.approval_consumption_max_age_seconds = 0;

    let error = check_operator_evidence_rejection(
        operator_evidence,
        "approval_consumption_max_age_seconds",
    )
    .await;

    assert!(
        matches!(
            error,
            BoltV3LiveCanaryGateError::InvalidApprovalConsumptionMaxAge { value: 0 }
        ),
        "expected approval consumption max-age rejection, got {error:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn live_canary_gate_rejects_oversized_operator_evidence_file_before_hashing() {
    let mut operator_evidence = valid_operator_evidence();
    operator_evidence.max_operator_evidence_file_bytes = 8;

    let error =
        check_operator_evidence_rejection(operator_evidence, "approval_envelope_sha256").await;

    match error {
        BoltV3LiveCanaryGateError::OperatorEvidenceRead { field, source, .. } => {
            assert_eq!(field, "approval_envelope_sha256");
            assert!(
                source.to_string().contains("exceeds"),
                "expected oversize read rejection, got {source}"
            );
        }
        other => panic!("expected oversized operator evidence read rejection, got {other:?}"),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn live_canary_gate_rejects_oversized_approval_consumption_before_reading_to_eof() {
    let mut operator_evidence = valid_operator_evidence();
    operator_evidence.max_operator_evidence_file_bytes =
        largest_pre_consumption_operator_evidence_file_len(&operator_evidence) + 1;
    std::fs::write(
        &operator_evidence.approval_consumption_path,
        vec![b' '; operator_evidence.max_operator_evidence_file_bytes as usize + 1],
    )
    .expect("oversized approval consumption proof should write");

    let error =
        check_operator_evidence_rejection(operator_evidence, "approval_consumption_path").await;

    match error {
        BoltV3LiveCanaryGateError::OperatorApprovalConsumptionRead { source, .. } => {
            assert!(
                source.to_string().contains("exceeds"),
                "expected oversize proof read rejection, got {source}"
            );
        }
        other => panic!("expected oversized approval consumption read rejection, got {other:?}"),
    }
}

fn largest_pre_consumption_operator_evidence_file_len(
    evidence: &LiveCanaryOperatorEvidenceBlock,
) -> u64 {
    // Derive the size bound from the production gate's single source of truth so
    // a newly-added bounded read (e.g. the `decision_evidence_path` chain) can
    // never desync the test's size bound from the gate's read accounting and let
    // a non-consumption file exceed the limit ahead of the consumption read.
    pre_consumption_operator_evidence_bounded_read_paths(evidence)
        .into_iter()
        .map(|path| {
            std::fs::metadata(path)
                .unwrap_or_else(|error| {
                    panic!("operator evidence file `{path}` should exist: {error}")
                })
                .len()
        })
        .max()
        .expect("operator evidence should include pre-consumption files")
}

#[tokio::test(flavor = "current_thread")]
async fn live_canary_gate_rejects_missing_approval_consumption_proof() {
    let loaded = support::loaded_bolt_v3_live_canary_with_satisfied_report(
        1,
        rust_decimal::Decimal::new(25, 2),
    );
    let approval_consumption_path = loaded
        .root
        .live_canary
        .as_ref()
        .and_then(|block| block.operator_evidence.as_ref())
        .expect("fixture should include operator evidence")
        .approval_consumption_path
        .clone();
    std::fs::remove_file(&approval_consumption_path)
        .expect("fixture should start with removable approval consumption proof");

    let error = check_bolt_v3_live_canary_gate(&loaded)
        .await
        .expect_err("production gate must reject missing approval consumption proof");

    match error {
        BoltV3LiveCanaryGateError::OperatorApprovalConsumptionRead { source, .. } => {
            assert_eq!(source.kind(), std::io::ErrorKind::NotFound);
        }
        other => panic!("expected missing approval consumption proof rejection, got {other:?}"),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn live_canary_pre_consumption_gate_accepts_without_pre_run_order_id_hashes() {
    let mut loaded = support::loaded_bolt_v3_live_canary_with_satisfied_report(
        1,
        rust_decimal::Decimal::new(25, 2),
    );
    let operator_evidence = loaded
        .root
        .live_canary
        .as_mut()
        .and_then(|block| block.operator_evidence.as_mut())
        .expect("fixture should include operator evidence");
    std::fs::remove_file(&operator_evidence.approval_consumption_path)
        .expect("fixture should start with removable approval consumption proof");

    let report = check_bolt_v3_live_canary_pre_consumption_gate(&loaded)
        .await
        .expect("pre-consumption gate must not require order IDs before live runner entry");

    assert_eq!(report.approval_id(), "operator-approved-canary-001");
}

#[tokio::test(flavor = "current_thread")]
async fn pre_consumption_gate_rejects_stale_source_owned_strategy_input_before_approval() {
    let mut loaded = support::loaded_bolt_v3_live_canary_with_satisfied_report(
        1,
        rust_decimal::Decimal::new(25, 2),
    );
    let reference_quote_max_age_seconds = loaded
        .root
        .live_canary
        .as_ref()
        .expect("fixture should include live_canary")
        .reference_quote_max_age_seconds;
    let operator_evidence = loaded
        .root
        .live_canary
        .as_mut()
        .and_then(|block| block.operator_evidence.as_mut())
        .expect("fixture should include operator evidence");
    make_strategy_input_reference_quote_stale(
        operator_evidence,
        reference_quote_max_age_seconds + 1,
    );
    std::fs::remove_file(&operator_evidence.approval_consumption_path)
        .expect("fixture should start with removable approval consumption proof");

    let error = check_bolt_v3_live_canary_pre_consumption_gate(&loaded)
        .await
        .expect_err(
            "stale source-owned strategy_input reference quote must fail before approval consumption",
        );

    assert!(
        error.to_string().contains("strategy_input_evidence")
            && error
                .to_string()
                .contains("reference_quote_max_age_seconds"),
        "expected stale source-owned strategy_input rejection, got {error:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn pre_consumption_gate_rejects_source_order_notional_above_live_cap_before_approval() {
    let mut loaded = support::loaded_bolt_v3_live_canary_with_satisfied_report(
        1,
        rust_decimal::Decimal::new(100, 2),
    );
    let operator_evidence = loaded
        .root
        .live_canary
        .as_mut()
        .and_then(|block| block.operator_evidence.as_mut())
        .expect("fixture should include operator evidence");
    make_latest_decision_evidence_notional(operator_evidence, "1.01");
    std::fs::remove_file(&operator_evidence.approval_consumption_path)
        .expect("fixture should start with removable approval consumption proof");

    let error = check_bolt_v3_live_canary_pre_consumption_gate(&loaded)
        .await
        .expect_err("source-owned order notional above canary cap must fail before approval");

    assert!(
        error.to_string().contains("decision_evidence")
            && error.to_string().contains("max_notional_per_order"),
        "expected source-owned notional cap rejection, got {error:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn live_canary_gate_rejects_missing_operator_evidence_file_before_hashing() {
    let mut operator_evidence = valid_operator_evidence();
    let missing = std::path::Path::new(&operator_evidence.ssm_manifest_path)
        .with_file_name("missing-ssm-manifest.json");
    operator_evidence.ssm_manifest_path = missing.to_string_lossy().to_string();

    let error = check_operator_evidence_rejection(operator_evidence, "ssm_manifest_sha256").await;

    match error {
        BoltV3LiveCanaryGateError::OperatorEvidenceRead { field, source, .. } => {
            assert_eq!(field, "ssm_manifest_sha256");
            assert_eq!(source.kind(), std::io::ErrorKind::NotFound);
        }
        other => panic!("expected missing operator evidence read rejection, got {other:?}"),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn live_canary_gate_rejects_non_regular_operator_evidence_path() {
    let mut operator_evidence = valid_operator_evidence();
    let target = std::path::Path::new(&operator_evidence.ssm_manifest_path);
    let symlink = target.with_file_name("ssm-manifest-link.json");
    #[cfg(unix)]
    std::os::unix::fs::symlink(target, &symlink).expect("test symlink should be created");
    #[cfg(windows)]
    std::os::windows::fs::symlink_file(target, &symlink).expect("test symlink should be created");
    operator_evidence.ssm_manifest_path = symlink.to_string_lossy().to_string();

    let error = check_operator_evidence_rejection(operator_evidence, "ssm_manifest_sha256").await;

    match error {
        BoltV3LiveCanaryGateError::OperatorEvidenceRead { field, source, .. } => {
            assert_eq!(field, "ssm_manifest_sha256");
            assert!(
                source.to_string().contains("regular file"),
                "expected non-regular file rejection, got {source}"
            );
        }
        other => panic!("expected non-regular operator evidence read rejection, got {other:?}"),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn live_canary_gate_rejects_symlinked_readiness_report_path() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let report_path = tempdir.path().join("no-submit-readiness.json");
    write_no_submit_report(&report_path, &[]);
    let symlink = tempdir.path().join("no-submit-readiness-link.json");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&report_path, &symlink).expect("test symlink should be created");
    #[cfg(windows)]
    std::os::windows::fs::symlink_file(&report_path, &symlink)
        .expect("test symlink should be created");
    let loaded = loaded_with_live_canary(
        loaded,
        LiveCanaryBlock {
            approval_id: "operator-approved-canary-001".to_string(),
            no_submit_readiness_report_path: symlink.to_string_lossy().to_string(),
            max_live_order_count: 1,
            max_notional_per_order: "1.00".to_string(),
            max_no_submit_readiness_report_bytes: 4096,
            readiness_report_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_wait_timeout_seconds: 10,
            reference_quote_probe_actor_id: "no-submit-reference-quote-probe".to_string(),
            reference_quote_probe_log_events: true,
            reference_quote_probe_log_commands: true,
            egress_identity_observed_path: None,
            egress_identity_observed_max_bytes: None,
            approved_egress_identity_sha256: None,
            proof_policy: None,
            operator_evidence: Some(valid_operator_evidence()),
        },
    );

    let error = check_bolt_v3_live_canary_gate(&loaded)
        .await
        .expect_err("symlinked no-submit readiness report must fail closed");

    match error {
        BoltV3LiveCanaryGateError::ReadinessReportRead { path, source } => {
            assert_eq!(path, symlink);
            assert!(
                source.to_string().contains("regular file"),
                "expected non-regular report rejection, got {source}"
            );
        }
        other => panic!("expected non-regular readiness report rejection, got {other:?}"),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn live_canary_gate_rejects_zero_order_count() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let loaded = loaded_with_live_canary(
        loaded,
        LiveCanaryBlock {
            approval_id: "operator-approved-canary-001".to_string(),
            no_submit_readiness_report_path: "not-read-before-order-count-check.json".to_string(),
            max_live_order_count: 0,
            max_notional_per_order: "1.00".to_string(),
            max_no_submit_readiness_report_bytes: 4096,
            readiness_report_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_wait_timeout_seconds: 10,
            reference_quote_probe_actor_id: "no-submit-reference-quote-probe".to_string(),
            reference_quote_probe_log_events: true,
            reference_quote_probe_log_commands: true,
            egress_identity_observed_path: None,
            egress_identity_observed_max_bytes: None,
            approved_egress_identity_sha256: None,
            proof_policy: None,
            operator_evidence: Some(valid_operator_evidence()),
        },
    );

    let error = check_bolt_v3_live_canary_gate(&loaded)
        .await
        .expect_err("zero max_live_order_count must fail closed");

    assert!(
        matches!(
            error,
            BoltV3LiveCanaryGateError::InvalidMaxLiveOrderCount { value: 0 }
        ),
        "expected order-count rejection, got {error:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn live_canary_gate_rejects_zero_report_byte_cap() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let loaded = loaded_with_live_canary(
        loaded,
        LiveCanaryBlock {
            approval_id: "operator-approved-canary-001".to_string(),
            no_submit_readiness_report_path: "not-read-before-size-limit-check.json".to_string(),
            max_live_order_count: 1,
            max_notional_per_order: "1.00".to_string(),
            max_no_submit_readiness_report_bytes: 0,
            readiness_report_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_wait_timeout_seconds: 10,
            reference_quote_probe_actor_id: "no-submit-reference-quote-probe".to_string(),
            reference_quote_probe_log_events: true,
            reference_quote_probe_log_commands: true,
            egress_identity_observed_path: None,
            egress_identity_observed_max_bytes: None,
            approved_egress_identity_sha256: None,
            proof_policy: None,
            operator_evidence: Some(valid_operator_evidence()),
        },
    );

    let error = check_bolt_v3_live_canary_gate(&loaded)
        .await
        .expect_err("zero readiness report byte cap must fail closed");

    assert!(
        matches!(
            error,
            BoltV3LiveCanaryGateError::InvalidReadinessReportSizeLimit { value: 0 }
        ),
        "expected readiness report byte-cap rejection, got {error:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn live_canary_gate_rejects_zero_readiness_report_max_age() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let loaded = loaded_with_live_canary(
        loaded,
        LiveCanaryBlock {
            approval_id: "operator-approved-canary-001".to_string(),
            no_submit_readiness_report_path: "not-read-before-max-age-check.json".to_string(),
            max_live_order_count: 1,
            max_notional_per_order: "1.00".to_string(),
            max_no_submit_readiness_report_bytes: 4096,
            readiness_report_max_age_seconds: 0,
            reference_quote_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_wait_timeout_seconds: 10,
            reference_quote_probe_actor_id: "no-submit-reference-quote-probe".to_string(),
            reference_quote_probe_log_events: true,
            reference_quote_probe_log_commands: true,
            egress_identity_observed_path: None,
            egress_identity_observed_max_bytes: None,
            approved_egress_identity_sha256: None,
            proof_policy: None,
            operator_evidence: Some(valid_operator_evidence()),
        },
    );

    let error = check_bolt_v3_live_canary_gate(&loaded)
        .await
        .expect_err("zero readiness report max age must fail closed");

    assert!(
        matches!(
            error,
            BoltV3LiveCanaryGateError::InvalidReadinessReportMaxAge { value: 0 }
        ),
        "expected readiness report max-age rejection, got {error:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn live_canary_gate_rejects_zero_reference_quote_max_age() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let loaded = loaded_with_live_canary(
        loaded,
        LiveCanaryBlock {
            approval_id: "operator-approved-canary-001".to_string(),
            no_submit_readiness_report_path: "not-read-before-reference-max-age-check.json"
                .to_string(),
            max_live_order_count: 1,
            max_notional_per_order: "1.00".to_string(),
            max_no_submit_readiness_report_bytes: 4096,
            readiness_report_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_max_age_seconds: 0,
            reference_quote_wait_timeout_seconds: 10,
            reference_quote_probe_actor_id: "no-submit-reference-quote-probe".to_string(),
            reference_quote_probe_log_events: true,
            reference_quote_probe_log_commands: true,
            egress_identity_observed_path: None,
            egress_identity_observed_max_bytes: None,
            approved_egress_identity_sha256: None,
            proof_policy: None,
            operator_evidence: Some(valid_operator_evidence()),
        },
    );

    let error = check_bolt_v3_live_canary_gate(&loaded)
        .await
        .expect_err("zero reference quote max age must fail closed");

    assert!(
        matches!(
            error,
            BoltV3LiveCanaryGateError::InvalidReferenceQuoteMaxAge { value: 0 }
        ),
        "expected reference quote max-age rejection, got {error:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn live_canary_gate_rejects_zero_reference_quote_wait_timeout() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let loaded = loaded_with_live_canary(
        loaded,
        LiveCanaryBlock {
            approval_id: "operator-approved-canary-001".to_string(),
            no_submit_readiness_report_path: "not-read-before-reference-wait-timeout-check.json"
                .to_string(),
            max_live_order_count: 1,
            max_notional_per_order: "1.00".to_string(),
            max_no_submit_readiness_report_bytes: 4096,
            readiness_report_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_wait_timeout_seconds: 0,
            reference_quote_probe_actor_id: "no-submit-reference-quote-probe".to_string(),
            reference_quote_probe_log_events: true,
            reference_quote_probe_log_commands: true,
            egress_identity_observed_path: None,
            egress_identity_observed_max_bytes: None,
            approved_egress_identity_sha256: None,
            proof_policy: None,
            operator_evidence: Some(valid_operator_evidence()),
        },
    );

    let error = check_bolt_v3_live_canary_gate(&loaded)
        .await
        .expect_err("zero reference quote wait timeout must fail closed");

    assert!(
        matches!(
            error,
            BoltV3LiveCanaryGateError::InvalidReferenceQuoteWaitTimeout { value: 0 }
        ),
        "expected reference quote wait-timeout rejection, got {error:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn live_canary_gate_rejects_blank_reference_quote_probe_actor_id() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let loaded = loaded_with_live_canary(
        loaded,
        LiveCanaryBlock {
            approval_id: "operator-approved-canary-001".to_string(),
            no_submit_readiness_report_path: "not-read-before-reference-probe-actor-id-check.json"
                .to_string(),
            max_live_order_count: 1,
            max_notional_per_order: "1.00".to_string(),
            max_no_submit_readiness_report_bytes: 4096,
            readiness_report_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_wait_timeout_seconds: 10,
            reference_quote_probe_actor_id: " ".to_string(),
            reference_quote_probe_log_events: true,
            reference_quote_probe_log_commands: true,
            egress_identity_observed_path: None,
            egress_identity_observed_max_bytes: None,
            approved_egress_identity_sha256: None,
            proof_policy: None,
            operator_evidence: Some(valid_operator_evidence()),
        },
    );

    let error = check_bolt_v3_live_canary_gate(&loaded)
        .await
        .expect_err("blank reference quote probe actor id must fail closed");

    assert!(
        matches!(
            error,
            BoltV3LiveCanaryGateError::InvalidReferenceQuoteProbeActorId { .. }
        ),
        "expected reference quote probe actor-id rejection, got {error:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn live_canary_gate_rejects_malformed_reference_quote_probe_actor_id() {
    for actor_id in [
        " no-submit-reference-quote-probe",
        "no-submit-reference-quote-probe ",
        "프로브",
    ] {
        let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
        let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
        let loaded = loaded_with_live_canary(
            loaded,
            LiveCanaryBlock {
                approval_id: "operator-approved-canary-001".to_string(),
                no_submit_readiness_report_path:
                    "not-read-before-reference-probe-actor-id-check.json".to_string(),
                max_live_order_count: 1,
                max_notional_per_order: "1.00".to_string(),
                max_no_submit_readiness_report_bytes: 4096,
                readiness_report_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
                reference_quote_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
                reference_quote_wait_timeout_seconds: 10,
                reference_quote_probe_actor_id: actor_id.to_string(),
                reference_quote_probe_log_events: true,
                reference_quote_probe_log_commands: true,
                egress_identity_observed_path: None,
                egress_identity_observed_max_bytes: None,
                approved_egress_identity_sha256: None,
                proof_policy: None,
                operator_evidence: Some(valid_operator_evidence()),
            },
        );

        let error = check_bolt_v3_live_canary_gate(&loaded)
            .await
            .expect_err("malformed reference quote probe actor id must fail closed");

        assert!(
            matches!(
                error,
                BoltV3LiveCanaryGateError::InvalidReferenceQuoteProbeActorId { .. }
            ),
            "expected reference quote probe actor-id rejection for {actor_id:?}, got {error:?}"
        );
    }
}

#[test]
fn live_canary_gate_uses_named_approval_consumption_protocol_constants() {
    let source = support::repo_text("src/bolt_v3_live_canary_gate.rs");
    let tiny_source = support::repo_text("src/bolt_v3_tiny_canary_evidence.rs");
    let schema_source = support::repo_text("src/bolt_v3_no_submit_readiness_schema.rs");

    assert!(
        schema_source.contains("pub const APPROVAL_CONSUMPTION_SCHEMA_VERSION"),
        "approval-consumption schema version must be a shared protocol constant"
    );
    assert!(
        schema_source.contains("pub const APPROVAL_CONSUMPTION_RECORD_KIND"),
        "approval-consumption record kind must be a shared protocol constant"
    );
    assert!(
        source.contains("APPROVAL_CONSUMPTION_SCHEMA_VERSION"),
        "gate validation must consume shared approval-consumption schema version"
    );
    assert!(
        tiny_source.contains("APPROVAL_CONSUMPTION_SCHEMA_VERSION"),
        "consumption evidence writer must consume shared approval-consumption schema version"
    );
    assert!(
        !source.contains("validate_consumption_i64_field(&path, object, \"schema_version\", 1)?;"),
        "approval-consumption schema version validation must not use an inline literal"
    );
    assert!(
        !source.contains("const APPROVAL_CONSUMPTION_SCHEMA_VERSION"),
        "approval-consumption constants must not be gate-local"
    );
    assert!(
        !tiny_source.contains("const PHASE8_APPROVAL_CONSUMPTION_SCHEMA_VERSION"),
        "approval-consumption constants must not be duplicated in the evidence writer"
    );
    assert_eq!(APPROVAL_CONSUMPTION_SCHEMA_VERSION, 1);
    assert_eq!(
        APPROVAL_CONSUMPTION_RECORD_KIND,
        "phase8_operator_approval_consumption"
    );
    assert!(
        !source.contains("\"phase8_operator_approval_consumption\",\n    )?;"),
        "approval-consumption record-kind validation must not use an inline literal"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn live_canary_gate_rejects_invalid_canary_notional_values() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");

    for candidate in ["abc", "0.00", "-1.00"] {
        let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
        let loaded = loaded_with_live_canary(
            loaded,
            LiveCanaryBlock {
                approval_id: "operator-approved-canary-001".to_string(),
                no_submit_readiness_report_path: "not-read-before-notional-check.json".to_string(),
                max_live_order_count: 1,
                max_notional_per_order: candidate.to_string(),
                max_no_submit_readiness_report_bytes: 4096,
                readiness_report_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
                reference_quote_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
                reference_quote_wait_timeout_seconds: 10,
                reference_quote_probe_actor_id: "no-submit-reference-quote-probe".to_string(),
                reference_quote_probe_log_events: true,
                reference_quote_probe_log_commands: true,
                egress_identity_observed_path: None,
                egress_identity_observed_max_bytes: None,
                approved_egress_identity_sha256: None,
                proof_policy: None,
                operator_evidence: Some(valid_operator_evidence()),
            },
        );

        let error = check_bolt_v3_live_canary_gate(&loaded)
            .await
            .expect_err("invalid canary notional must fail closed");

        match error {
            BoltV3LiveCanaryGateError::InvalidMaxNotional { field, value, .. } => {
                assert_eq!(field, "max_notional_per_order");
                assert_eq!(value, candidate);
            }
            other => panic!("expected invalid canary notional rejection, got {other:?}"),
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn live_canary_gate_rejects_invalid_root_notional_values() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");

    for candidate in ["abc", "0.00", "-1.00"] {
        let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
        loaded.root.risk.default_max_notional_per_order = candidate.to_string();
        let loaded = loaded_with_live_canary(
            loaded,
            LiveCanaryBlock {
                approval_id: "operator-approved-canary-001".to_string(),
                no_submit_readiness_report_path: "not-read-before-root-notional-check.json"
                    .to_string(),
                max_live_order_count: 1,
                max_notional_per_order: "1.00".to_string(),
                max_no_submit_readiness_report_bytes: 4096,
                readiness_report_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
                reference_quote_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
                reference_quote_wait_timeout_seconds: 10,
                reference_quote_probe_actor_id: "no-submit-reference-quote-probe".to_string(),
                reference_quote_probe_log_events: true,
                reference_quote_probe_log_commands: true,
                egress_identity_observed_path: None,
                egress_identity_observed_max_bytes: None,
                approved_egress_identity_sha256: None,
                proof_policy: None,
                operator_evidence: Some(valid_operator_evidence()),
            },
        );

        let error = check_bolt_v3_live_canary_gate(&loaded)
            .await
            .expect_err("invalid root notional must fail closed");

        match error {
            BoltV3LiveCanaryGateError::InvalidMaxNotional { field, value, .. } => {
                assert_eq!(field, "risk.default_max_notional_per_order");
                assert_eq!(value, candidate);
            }
            other => panic!("expected invalid root notional rejection, got {other:?}"),
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn live_canary_gate_accepts_satisfied_no_submit_report_with_trimmed_capped_notional() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    loaded.root.risk.default_max_notional_per_order = " 10.00 ".to_string();
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let report_path = tempdir.path().join("no-submit-readiness.json");
    write_no_submit_report(&report_path, &[]);
    let mut operator_evidence = valid_operator_evidence();
    // Seal the genuine readiness-report hash into the envelope: the report
    // binding is mandatory on the production (proof-disabled) path too.
    seal_no_submit_readiness_report_into_production_evidence(&mut operator_evidence, &report_path);

    let loaded = loaded_with_live_canary(
        loaded,
        LiveCanaryBlock {
            approval_id: "operator-approved-canary-001".to_string(),
            no_submit_readiness_report_path: report_path.to_string_lossy().to_string(),
            max_live_order_count: 1,
            max_notional_per_order: " 1.00 ".to_string(),
            max_no_submit_readiness_report_bytes: 4096,
            readiness_report_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_wait_timeout_seconds: 10,
            reference_quote_probe_actor_id: "no-submit-reference-quote-probe".to_string(),
            reference_quote_probe_log_events: true,
            reference_quote_probe_log_commands: true,
            egress_identity_observed_path: None,
            egress_identity_observed_max_bytes: None,
            approved_egress_identity_sha256: None,
            proof_policy: None,
            operator_evidence: Some(operator_evidence),
        },
    );

    let report = check_bolt_v3_live_canary_gate(&loaded)
        .await
        .expect("satisfied no-submit report and capped notional should pass");

    assert_eq!(report.approval_id(), "operator-approved-canary-001");
    assert_eq!(report.max_live_order_count(), 1);
    assert_eq!(
        report.readiness_report_max_age_seconds(),
        TEST_READINESS_REPORT_MAX_AGE_SECONDS
    );
    assert_eq!(
        report.no_submit_readiness_report_path(),
        report_path.as_path(),
        "absolute report path should be preserved"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn live_canary_gate_accepts_notional_equal_to_root_risk_cap() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    loaded.root.risk.default_max_notional_per_order = "10.00".to_string();
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let report_path = tempdir.path().join("no-submit-readiness.json");
    write_no_submit_report(&report_path, &[]);
    let mut operator_evidence = valid_operator_evidence();
    // Seal the genuine readiness-report hash into the envelope: the report
    // binding is mandatory on the production (proof-disabled) path too.
    seal_no_submit_readiness_report_into_production_evidence(&mut operator_evidence, &report_path);

    let loaded = loaded_with_live_canary(
        loaded,
        LiveCanaryBlock {
            approval_id: "operator-approved-canary-001".to_string(),
            no_submit_readiness_report_path: report_path.to_string_lossy().to_string(),
            max_live_order_count: 1,
            max_notional_per_order: "10.00".to_string(),
            max_no_submit_readiness_report_bytes: 4096,
            readiness_report_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_wait_timeout_seconds: 10,
            reference_quote_probe_actor_id: "no-submit-reference-quote-probe".to_string(),
            reference_quote_probe_log_events: true,
            reference_quote_probe_log_commands: true,
            egress_identity_observed_path: None,
            egress_identity_observed_max_bytes: None,
            approved_egress_identity_sha256: None,
            proof_policy: None,
            operator_evidence: Some(operator_evidence),
        },
    );

    check_bolt_v3_live_canary_gate(&loaded)
        .await
        .expect("notional equal to root risk cap should pass");
}

#[tokio::test(flavor = "current_thread")]
async fn live_canary_gate_rejects_report_expired_at_late_gate_timestamp() {
    let now = current_unix_seconds_for_test();
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let report_path = tempdir.path().join("no-submit-readiness.json");
    write_no_submit_report_at(
        &report_path,
        &[],
        now - TEST_READINESS_REPORT_MAX_AGE_SECONDS - 1,
    );
    let loaded = loaded_with_live_canary(
        loaded,
        LiveCanaryBlock {
            approval_id: "operator-approved-canary-001".to_string(),
            no_submit_readiness_report_path: report_path.to_string_lossy().to_string(),
            max_live_order_count: 1,
            max_notional_per_order: "1.00".to_string(),
            max_no_submit_readiness_report_bytes: 4096,
            readiness_report_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_wait_timeout_seconds: 10,
            reference_quote_probe_actor_id: "no-submit-reference-quote-probe".to_string(),
            reference_quote_probe_log_events: true,
            reference_quote_probe_log_commands: true,
            egress_identity_observed_path: None,
            egress_identity_observed_max_bytes: None,
            approved_egress_identity_sha256: None,
            proof_policy: None,
            operator_evidence: Some(valid_operator_evidence_for_window(
                now as i64 - 10,
                now as i64 + 3600,
            )),
        },
    );

    let error = check_bolt_v3_live_canary_gate(&loaded)
        .await
        .expect_err("stale readiness report must fail closed");

    match error {
        BoltV3LiveCanaryGateError::UnsatisfiedNoSubmitReadinessReport { reasons, .. } => {
            assert!(
                reasons
                    .iter()
                    .any(|reason| reason.contains("generated_at_unix_seconds expired")),
                "expected generated_at expiry reason, got {reasons:?}"
            );
        }
        other => panic!("expected expired readiness report rejection, got {other:?}"),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn live_canary_gate_rejects_operator_window_expired_at_gate_timestamp() {
    let now = current_unix_seconds_for_test();
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let report_path = tempdir.path().join("no-submit-readiness.json");
    write_no_submit_report_at(&report_path, &[], now);
    let loaded = loaded_with_live_canary(
        loaded,
        LiveCanaryBlock {
            approval_id: "operator-approved-canary-001".to_string(),
            no_submit_readiness_report_path: report_path.to_string_lossy().to_string(),
            max_live_order_count: 1,
            max_notional_per_order: "1.00".to_string(),
            max_no_submit_readiness_report_bytes: 4096,
            readiness_report_max_age_seconds: 3_600,
            reference_quote_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_wait_timeout_seconds: 10,
            reference_quote_probe_actor_id: "no-submit-reference-quote-probe".to_string(),
            reference_quote_probe_log_events: true,
            reference_quote_probe_log_commands: true,
            egress_identity_observed_path: None,
            egress_identity_observed_max_bytes: None,
            approved_egress_identity_sha256: None,
            proof_policy: None,
            operator_evidence: Some(valid_operator_evidence_for_window(
                now as i64 - 120,
                now as i64 - 60,
            )),
        },
    );

    let error = check_bolt_v3_live_canary_gate(&loaded)
        .await
        .expect_err("expired operator approval window must fail closed");

    match error {
        BoltV3LiveCanaryGateError::InactiveOperatorApprovalWindow {
            current_unix_seconds,
            approval_not_after_unix_seconds,
            ..
        } => {
            assert!(current_unix_seconds >= now);
            assert_eq!(approval_not_after_unix_seconds, now as i64 - 60);
        }
        other => panic!("expected operator approval window rejection, got {other:?}"),
    }
}

type OperatorEvidenceStringMutator = fn(&mut LiveCanaryOperatorEvidenceBlock);

#[tokio::test(flavor = "current_thread")]
async fn live_canary_gate_rejects_blank_operator_evidence_string_fields() {
    let cases: &[(&str, OperatorEvidenceStringMutator)] = &[
        ("approval_envelope_path", |e| {
            e.approval_envelope_path = blank_operator_evidence_value()
        }),
        ("ssm_manifest_path", |e| {
            e.ssm_manifest_path = blank_operator_evidence_value()
        }),
        ("ssm_manifest_sha256", |e| {
            e.ssm_manifest_sha256 = blank_operator_evidence_value()
        }),
        ("strategy_input_evidence_path", |e| {
            e.strategy_input_evidence_path = blank_operator_evidence_value()
        }),
        ("strategy_input_evidence_sha256", |e| {
            e.strategy_input_evidence_sha256 = blank_operator_evidence_value()
        }),
        ("financial_envelope_path", |e| {
            e.financial_envelope_path = blank_operator_evidence_value()
        }),
        ("financial_envelope_sha256", |e| {
            e.financial_envelope_sha256 = blank_operator_evidence_value()
        }),
        ("pre_run_state_path", |e| {
            e.pre_run_state_path = blank_operator_evidence_value()
        }),
        ("pre_run_state_sha256", |e| {
            e.pre_run_state_sha256 = blank_operator_evidence_value()
        }),
        ("abort_plan_path", |e| {
            e.abort_plan_path = blank_operator_evidence_value()
        }),
        ("abort_plan_sha256", |e| {
            e.abort_plan_sha256 = blank_operator_evidence_value()
        }),
        ("canary_evidence_path", |e| {
            e.canary_evidence_path = blank_operator_evidence_value()
        }),
        ("approval_nonce_path", |e| {
            e.approval_nonce_path = blank_operator_evidence_value()
        }),
        ("approval_nonce_sha256", |e| {
            e.approval_nonce_sha256 = blank_operator_evidence_value()
        }),
        ("approval_consumption_path", |e| {
            e.approval_consumption_path = blank_operator_evidence_value()
        }),
        ("decision_evidence_path", |e| {
            e.decision_evidence_path = blank_operator_evidence_value()
        }),
        ("nt_submit_event_path", |e| {
            e.nt_submit_event_path = blank_operator_evidence_value()
        }),
        ("venue_order_state_path", |e| {
            e.venue_order_state_path = blank_operator_evidence_value()
        }),
        ("strategy_cancel_path", |e| {
            e.strategy_cancel_path = Some(blank_operator_evidence_value())
        }),
        ("restart_reconciliation_path", |e| {
            e.restart_reconciliation_path = blank_operator_evidence_value()
        }),
        ("post_run_hygiene_path", |e| {
            e.post_run_hygiene_path = blank_operator_evidence_value()
        }),
    ];

    for &(expected_field, mutate) in cases {
        let mut operator_evidence = valid_operator_evidence();
        mutate(&mut operator_evidence);

        let error = check_operator_evidence_rejection(operator_evidence, expected_field).await;

        match error {
            BoltV3LiveCanaryGateError::MissingOperatorEvidenceField { field } => {
                assert_eq!(field, expected_field);
            }
            other => panic!("expected {expected_field} rejection, got {other:?}"),
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn live_canary_gate_rejects_operator_evidence_window_without_positive_duration() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let report_path = tempdir.path().join("no-submit-readiness.json");
    write_no_submit_report(&report_path, &[]);
    let mut operator_evidence = valid_operator_evidence();
    operator_evidence.approval_not_before_unix_seconds = 1_000;
    operator_evidence.approval_not_after_unix_seconds = 1_000;
    let loaded = loaded_with_live_canary(
        loaded,
        LiveCanaryBlock {
            approval_id: "operator-approved-canary-001".to_string(),
            no_submit_readiness_report_path: report_path.to_string_lossy().to_string(),
            max_live_order_count: 1,
            max_notional_per_order: "1.00".to_string(),
            max_no_submit_readiness_report_bytes: 4096,
            readiness_report_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_wait_timeout_seconds: 10,
            reference_quote_probe_actor_id: "no-submit-reference-quote-probe".to_string(),
            reference_quote_probe_log_events: true,
            reference_quote_probe_log_commands: true,
            egress_identity_observed_path: None,
            egress_identity_observed_max_bytes: None,
            approved_egress_identity_sha256: None,
            proof_policy: None,
            operator_evidence: Some(operator_evidence),
        },
    );

    let error = check_bolt_v3_live_canary_gate(&loaded)
        .await
        .expect_err("non-positive operator approval window must fail closed");

    match error {
        BoltV3LiveCanaryGateError::InvalidOperatorApprovalWindow {
            approval_not_before_unix_seconds,
            approval_not_after_unix_seconds,
        } => {
            assert_eq!(approval_not_before_unix_seconds, 1_000);
            assert_eq!(approval_not_after_unix_seconds, 1_000);
        }
        other => panic!("expected invalid approval window rejection, got {other:?}"),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn live_canary_gate_rejects_operator_evidence_window_before_current_time() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let report_path = tempdir.path().join("no-submit-readiness.json");
    write_no_submit_report(&report_path, &[]);
    let now = current_unix_seconds_for_test() as i64;
    let mut operator_evidence = valid_operator_evidence();
    operator_evidence.approval_not_before_unix_seconds = now + 3600;
    operator_evidence.approval_not_after_unix_seconds = now + 7200;
    let loaded = loaded_with_live_canary(
        loaded,
        LiveCanaryBlock {
            approval_id: "operator-approved-canary-001".to_string(),
            no_submit_readiness_report_path: report_path.to_string_lossy().to_string(),
            max_live_order_count: 1,
            max_notional_per_order: "1.00".to_string(),
            max_no_submit_readiness_report_bytes: 4096,
            readiness_report_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_wait_timeout_seconds: 10,
            reference_quote_probe_actor_id: "no-submit-reference-quote-probe".to_string(),
            reference_quote_probe_log_events: true,
            reference_quote_probe_log_commands: true,
            egress_identity_observed_path: None,
            egress_identity_observed_max_bytes: None,
            approved_egress_identity_sha256: None,
            proof_policy: None,
            operator_evidence: Some(operator_evidence),
        },
    );

    let error = check_bolt_v3_live_canary_gate(&loaded)
        .await
        .expect_err("future operator approval window must fail closed");

    match error {
        BoltV3LiveCanaryGateError::InactiveOperatorApprovalWindow {
            approval_not_before_unix_seconds,
            approval_not_after_unix_seconds,
            ..
        } => {
            assert_eq!(approval_not_before_unix_seconds, now + 3600);
            assert_eq!(approval_not_after_unix_seconds, now + 7200);
        }
        other => panic!("expected inactive approval window rejection, got {other:?}"),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn live_canary_gate_rejects_operator_evidence_window_after_current_time() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let report_path = tempdir.path().join("no-submit-readiness.json");
    write_no_submit_report(&report_path, &[]);
    let now = current_unix_seconds_for_test() as i64;
    let mut operator_evidence = valid_operator_evidence();
    operator_evidence.approval_not_before_unix_seconds = now - 7200;
    operator_evidence.approval_not_after_unix_seconds = now - 3600;
    let loaded = loaded_with_live_canary(
        loaded,
        LiveCanaryBlock {
            approval_id: "operator-approved-canary-001".to_string(),
            no_submit_readiness_report_path: report_path.to_string_lossy().to_string(),
            max_live_order_count: 1,
            max_notional_per_order: "1.00".to_string(),
            max_no_submit_readiness_report_bytes: 4096,
            readiness_report_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_wait_timeout_seconds: 10,
            reference_quote_probe_actor_id: "no-submit-reference-quote-probe".to_string(),
            reference_quote_probe_log_events: true,
            reference_quote_probe_log_commands: true,
            egress_identity_observed_path: None,
            egress_identity_observed_max_bytes: None,
            approved_egress_identity_sha256: None,
            proof_policy: None,
            operator_evidence: Some(operator_evidence),
        },
    );

    let error = check_bolt_v3_live_canary_gate(&loaded)
        .await
        .expect_err("expired operator approval window must fail closed");

    match error {
        BoltV3LiveCanaryGateError::InactiveOperatorApprovalWindow {
            approval_not_before_unix_seconds,
            approval_not_after_unix_seconds,
            ..
        } => {
            assert_eq!(approval_not_before_unix_seconds, now - 7200);
            assert_eq!(approval_not_after_unix_seconds, now - 3600);
        }
        other => panic!("expected inactive approval window rejection, got {other:?}"),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn live_canary_gate_rejects_stale_no_submit_linkage_fields() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let report_path = tempdir.path().join("no-submit-readiness.json");
    write_no_submit_report_with_linkage(
        &report_path,
        "stale-approval-id-hash",
        "stale-executable-identity",
        "stale-config-bundle-checksum",
    );

    let loaded = loaded_with_live_canary(
        loaded,
        LiveCanaryBlock {
            approval_id: "operator-approved-canary-001".to_string(),
            no_submit_readiness_report_path: report_path.to_string_lossy().to_string(),
            max_live_order_count: 1,
            max_notional_per_order: "1.00".to_string(),
            max_no_submit_readiness_report_bytes: 4096,
            readiness_report_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_wait_timeout_seconds: 10,
            reference_quote_probe_actor_id: "no-submit-reference-quote-probe".to_string(),
            reference_quote_probe_log_events: true,
            reference_quote_probe_log_commands: true,
            egress_identity_observed_path: None,
            egress_identity_observed_max_bytes: None,
            approved_egress_identity_sha256: None,
            proof_policy: None,
            operator_evidence: Some(valid_operator_evidence()),
        },
    );

    let error = check_bolt_v3_live_canary_gate(&loaded)
        .await
        .expect_err("stale no-submit linkage must fail closed");

    match error {
        BoltV3LiveCanaryGateError::UnsatisfiedNoSubmitReadinessReport { reasons, .. } => {
            assert!(
                reasons
                    .iter()
                    .any(|reason| reason.contains(APPROVAL_ID_HASH_KEY))
                    && reasons
                        .iter()
                        .any(|reason| reason.contains(EXECUTABLE_IDENTITY_KEY))
                    && reasons
                        .iter()
                        .any(|reason| reason.contains(CONFIG_BUNDLE_CHECKSUM_KEY)),
                "stale linkage should report all linkage fields, got {reasons:?}"
            );
        }
        other => panic!("expected unsatisfied no-submit report rejection, got {other:?}"),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn live_canary_gate_rejects_no_submit_report_missing_generated_at() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let report_path = tempdir.path().join("no-submit-readiness.json");
    let mut report = linked_report_object(complete_stage_values());
    report.remove(GENERATED_AT_UNIX_SECONDS_KEY);
    write_report_value(&report_path, serde_json::Value::Object(report));

    let loaded = loaded_with_live_canary(
        loaded,
        LiveCanaryBlock {
            approval_id: "operator-approved-canary-001".to_string(),
            no_submit_readiness_report_path: report_path.to_string_lossy().to_string(),
            max_live_order_count: 1,
            max_notional_per_order: "1.00".to_string(),
            max_no_submit_readiness_report_bytes: 4096,
            readiness_report_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_wait_timeout_seconds: 10,
            reference_quote_probe_actor_id: "no-submit-reference-quote-probe".to_string(),
            reference_quote_probe_log_events: true,
            reference_quote_probe_log_commands: true,
            egress_identity_observed_path: None,
            egress_identity_observed_max_bytes: None,
            approved_egress_identity_sha256: None,
            proof_policy: None,
            operator_evidence: Some(valid_operator_evidence()),
        },
    );

    let error = check_bolt_v3_live_canary_gate(&loaded)
        .await
        .expect_err("missing generated_at_unix_seconds must fail closed");

    match error {
        BoltV3LiveCanaryGateError::UnsatisfiedNoSubmitReadinessReport { reasons, .. } => assert!(
            reasons
                .iter()
                .any(|reason| reason.contains("generated_at_unix_seconds")),
            "missing generated_at_unix_seconds should be reported, got {reasons:?}"
        ),
        other => panic!("expected unsatisfied no-submit report rejection, got {other:?}"),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn live_canary_gate_rejects_expired_no_submit_report() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let report_path = tempdir.path().join("no-submit-readiness.json");
    let generated_at =
        current_unix_seconds_for_test().saturating_sub(TEST_READINESS_REPORT_MAX_AGE_SECONDS + 1);
    write_no_submit_report_at(&report_path, &[], generated_at);

    let loaded = loaded_with_live_canary(
        loaded,
        LiveCanaryBlock {
            approval_id: "operator-approved-canary-001".to_string(),
            no_submit_readiness_report_path: report_path.to_string_lossy().to_string(),
            max_live_order_count: 1,
            max_notional_per_order: "1.00".to_string(),
            max_no_submit_readiness_report_bytes: 4096,
            readiness_report_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_wait_timeout_seconds: 10,
            reference_quote_probe_actor_id: "no-submit-reference-quote-probe".to_string(),
            reference_quote_probe_log_events: true,
            reference_quote_probe_log_commands: true,
            egress_identity_observed_path: None,
            egress_identity_observed_max_bytes: None,
            approved_egress_identity_sha256: None,
            proof_policy: None,
            operator_evidence: Some(valid_operator_evidence()),
        },
    );

    let error = check_bolt_v3_live_canary_gate(&loaded)
        .await
        .expect_err("expired no-submit report must fail closed");

    match error {
        BoltV3LiveCanaryGateError::UnsatisfiedNoSubmitReadinessReport { reasons, .. } => assert!(
            reasons.iter().any(|reason| reason.contains("expired")),
            "expired generated_at_unix_seconds should be reported, got {reasons:?}"
        ),
        other => panic!("expected unsatisfied no-submit report rejection, got {other:?}"),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn live_canary_gate_uses_toml_owned_readiness_report_max_age_seconds() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let report_path = tempdir.path().join("no-submit-readiness.json");
    let configured_max_age_seconds = 4;
    let generated_at =
        current_unix_seconds_for_test().saturating_sub(configured_max_age_seconds + 1);
    write_no_submit_report_at(&report_path, &[], generated_at);

    let loaded = loaded_with_live_canary(
        loaded,
        LiveCanaryBlock {
            approval_id: "operator-approved-canary-001".to_string(),
            no_submit_readiness_report_path: report_path.to_string_lossy().to_string(),
            max_live_order_count: 1,
            max_notional_per_order: "1.00".to_string(),
            max_no_submit_readiness_report_bytes: 4096,
            readiness_report_max_age_seconds: configured_max_age_seconds,
            reference_quote_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_wait_timeout_seconds: 10,
            reference_quote_probe_actor_id: "no-submit-reference-quote-probe".to_string(),
            reference_quote_probe_log_events: true,
            reference_quote_probe_log_commands: true,
            egress_identity_observed_path: None,
            egress_identity_observed_max_bytes: None,
            approved_egress_identity_sha256: None,
            proof_policy: None,
            operator_evidence: Some(valid_operator_evidence()),
        },
    );

    let error = check_bolt_v3_live_canary_gate(&loaded)
        .await
        .expect_err("report older than the configured max age must fail closed");

    match error {
        BoltV3LiveCanaryGateError::UnsatisfiedNoSubmitReadinessReport { reasons, .. } => assert!(
            reasons.iter().any(|reason| {
                reason.contains("readiness_report_max_age_seconds")
                    && reason.contains(&configured_max_age_seconds.to_string())
            }),
            "configured max age should be reported, got {reasons:?}"
        ),
        other => panic!("expected unsatisfied no-submit report rejection, got {other:?}"),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn live_canary_gate_rejects_missing_wrong_or_non_string_schema_version() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let cases = [
        ("missing", None, None),
        (
            "wrong",
            Some(serde_json::json!("bolt-v3.no-submit-readiness.v1")),
            Some("bolt-v3.no-submit-readiness.v1"),
        ),
        ("number", Some(serde_json::json!(2)), None),
    ];

    for (case_name, schema_version, expected_actual) in cases {
        let report_path = tempdir.path().join(format!("{case_name}.json"));
        let mut report = linked_report_object(complete_stage_values());
        match schema_version {
            Some(value) => {
                report.insert(SCHEMA_VERSION_KEY.to_string(), value);
            }
            None => {
                report.remove(SCHEMA_VERSION_KEY);
            }
        }
        write_report_value(&report_path, serde_json::Value::Object(report));
        let loaded = loaded_with_live_canary(
            loaded.clone(),
            LiveCanaryBlock {
                approval_id: "operator-approved-canary-001".to_string(),
                no_submit_readiness_report_path: report_path.to_string_lossy().to_string(),
                max_live_order_count: 1,
                max_notional_per_order: "1.00".to_string(),
                max_no_submit_readiness_report_bytes: 4096,
                readiness_report_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
                reference_quote_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
                reference_quote_wait_timeout_seconds: 10,
                reference_quote_probe_actor_id: "no-submit-reference-quote-probe".to_string(),
                reference_quote_probe_log_events: true,
                reference_quote_probe_log_commands: true,
                egress_identity_observed_path: None,
                egress_identity_observed_max_bytes: None,
                approved_egress_identity_sha256: None,
                proof_policy: None,
                operator_evidence: Some(valid_operator_evidence()),
            },
        );

        let error = check_bolt_v3_live_canary_gate(&loaded)
            .await
            .expect_err("schema-version mismatch must fail closed");

        match error {
            BoltV3LiveCanaryGateError::ReadinessReportSchemaVersionMismatch {
                expected,
                actual,
                ..
            } => {
                assert_eq!(expected, NO_SUBMIT_READINESS_SCHEMA_VERSION);
                assert_eq!(actual.as_deref(), expected_actual, "case={case_name}");
            }
            other => panic!("expected schema-version mismatch rejection, got {other:?}"),
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn live_canary_gate_rejects_notional_above_root_risk_cap() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let loaded = loaded_with_live_canary(
        loaded,
        LiveCanaryBlock {
            approval_id: "operator-approved-canary-001".to_string(),
            no_submit_readiness_report_path: "not-read-before-cap-check.json".to_string(),
            max_live_order_count: 1,
            max_notional_per_order: "11.00".to_string(),
            max_no_submit_readiness_report_bytes: 4096,
            readiness_report_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_wait_timeout_seconds: 10,
            reference_quote_probe_actor_id: "no-submit-reference-quote-probe".to_string(),
            reference_quote_probe_log_events: true,
            reference_quote_probe_log_commands: true,
            egress_identity_observed_path: None,
            egress_identity_observed_max_bytes: None,
            approved_egress_identity_sha256: None,
            proof_policy: None,
            operator_evidence: Some(valid_operator_evidence()),
        },
    );

    let error = check_bolt_v3_live_canary_gate(&loaded)
        .await
        .expect_err("canary notional above root risk cap must fail closed");

    assert!(
        matches!(
            error,
            BoltV3LiveCanaryGateError::MaxNotionalExceedsRootRisk { .. }
        ),
        "expected root risk cap rejection, got {error:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn live_canary_gate_rejects_empty_stage_report() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let report_path = tempdir.path().join("no-submit-readiness.json");
    write_linked_report_value(&report_path, serde_json::json!({ STAGES_KEY: [] }));
    let loaded = loaded_with_live_canary(
        loaded,
        LiveCanaryBlock {
            approval_id: "operator-approved-canary-001".to_string(),
            no_submit_readiness_report_path: report_path.to_string_lossy().to_string(),
            max_live_order_count: 1,
            max_notional_per_order: "1.00".to_string(),
            max_no_submit_readiness_report_bytes: 4096,
            readiness_report_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_wait_timeout_seconds: 10,
            reference_quote_probe_actor_id: "no-submit-reference-quote-probe".to_string(),
            reference_quote_probe_log_events: true,
            reference_quote_probe_log_commands: true,
            egress_identity_observed_path: None,
            egress_identity_observed_max_bytes: None,
            approved_egress_identity_sha256: None,
            proof_policy: None,
            operator_evidence: Some(valid_operator_evidence()),
        },
    );

    let error = check_bolt_v3_live_canary_gate(&loaded)
        .await
        .expect_err("empty no-submit stage report must fail closed");

    match error {
        BoltV3LiveCanaryGateError::UnsatisfiedNoSubmitReadinessReport { reasons, .. } => {
            assert!(
                reasons.iter().any(|reason| reason.contains("empty")),
                "error should name the empty stages array, got {reasons:?}"
            );
        }
        other => panic!("expected unsatisfied report rejection, got {other:?}"),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn live_canary_gate_rejects_report_missing_stages_key() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let report_path = tempdir.path().join("no-submit-readiness.json");
    write_linked_report_value(&report_path, serde_json::json!({ "other": true }));
    let loaded = loaded_with_live_canary(
        loaded,
        LiveCanaryBlock {
            approval_id: "operator-approved-canary-001".to_string(),
            no_submit_readiness_report_path: report_path.to_string_lossy().to_string(),
            max_live_order_count: 1,
            max_notional_per_order: "1.00".to_string(),
            max_no_submit_readiness_report_bytes: 4096,
            readiness_report_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_wait_timeout_seconds: 10,
            reference_quote_probe_actor_id: "no-submit-reference-quote-probe".to_string(),
            reference_quote_probe_log_events: true,
            reference_quote_probe_log_commands: true,
            egress_identity_observed_path: None,
            egress_identity_observed_max_bytes: None,
            approved_egress_identity_sha256: None,
            proof_policy: None,
            operator_evidence: Some(valid_operator_evidence()),
        },
    );

    let error = check_bolt_v3_live_canary_gate(&loaded)
        .await
        .expect_err("missing stages key must fail closed");

    match error {
        BoltV3LiveCanaryGateError::UnsatisfiedNoSubmitReadinessReport { reasons, .. } => {
            assert!(
                reasons
                    .iter()
                    .any(|reason| reason.contains("stages array is missing")),
                "error should name the missing stages array, got {reasons:?}"
            );
        }
        other => panic!("expected unsatisfied report rejection, got {other:?}"),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn live_canary_gate_rejects_unsatisfied_no_submit_report() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let report_path = tempdir.path().join("no-submit-readiness.json");
    write_no_submit_report(&report_path, &[(CONTROLLED_DISCONNECT_STAGE, "blocked")]);
    let loaded = loaded_with_live_canary(
        loaded,
        LiveCanaryBlock {
            approval_id: "operator-approved-canary-001".to_string(),
            no_submit_readiness_report_path: report_path.to_string_lossy().to_string(),
            max_live_order_count: 1,
            max_notional_per_order: "1.00".to_string(),
            max_no_submit_readiness_report_bytes: 4096,
            readiness_report_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_wait_timeout_seconds: 10,
            reference_quote_probe_actor_id: "no-submit-reference-quote-probe".to_string(),
            reference_quote_probe_log_events: true,
            reference_quote_probe_log_commands: true,
            egress_identity_observed_path: None,
            egress_identity_observed_max_bytes: None,
            approved_egress_identity_sha256: None,
            proof_policy: None,
            operator_evidence: Some(valid_operator_evidence()),
        },
    );

    let error = check_bolt_v3_live_canary_gate(&loaded)
        .await
        .expect_err("unsatisfied no-submit report must fail closed");

    match error {
        BoltV3LiveCanaryGateError::UnsatisfiedNoSubmitReadinessReport { reasons, .. } => {
            assert!(
                reasons
                    .iter()
                    .any(|reason| reason.contains(CONTROLLED_DISCONNECT_STAGE)),
                "error should name the blocked stage, got {reasons:?}"
            );
        }
        other => panic!("expected unsatisfied report rejection, got {other:?}"),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn live_canary_gate_reports_each_unsatisfied_required_stage_once() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let report_path = tempdir.path().join("no-submit-readiness.json");
    write_no_submit_report(&report_path, &[(REFERENCE_READINESS_STAGE, "skipped")]);
    let loaded = loaded_with_live_canary(
        loaded,
        LiveCanaryBlock {
            approval_id: "operator-approved-canary-001".to_string(),
            no_submit_readiness_report_path: report_path.to_string_lossy().to_string(),
            max_live_order_count: 1,
            max_notional_per_order: "1.00".to_string(),
            max_no_submit_readiness_report_bytes: 4096,
            readiness_report_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_wait_timeout_seconds: 10,
            reference_quote_probe_actor_id: "no-submit-reference-quote-probe".to_string(),
            reference_quote_probe_log_events: true,
            reference_quote_probe_log_commands: true,
            egress_identity_observed_path: None,
            egress_identity_observed_max_bytes: None,
            approved_egress_identity_sha256: None,
            proof_policy: None,
            operator_evidence: Some(valid_operator_evidence()),
        },
    );

    let error = check_bolt_v3_live_canary_gate(&loaded)
        .await
        .expect_err("unsatisfied required stage must fail closed");

    match error {
        BoltV3LiveCanaryGateError::UnsatisfiedNoSubmitReadinessReport { reasons, .. } => {
            let reference_readiness_reasons = reasons
                .iter()
                .filter(|reason| reason.contains(REFERENCE_READINESS_STAGE))
                .count();
            assert_eq!(
                reference_readiness_reasons, 1,
                "expected one reason for `{REFERENCE_READINESS_STAGE}`, got {reasons:?}"
            );
        }
        other => panic!("expected unsatisfied report rejection, got {other:?}"),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn live_canary_gate_rejects_missing_no_submit_report() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let report_path = tempdir.path().join("missing-no-submit-readiness.json");
    let loaded = loaded_with_live_canary(
        loaded,
        LiveCanaryBlock {
            approval_id: "operator-approved-canary-001".to_string(),
            no_submit_readiness_report_path: report_path.to_string_lossy().to_string(),
            max_live_order_count: 1,
            max_notional_per_order: "1.00".to_string(),
            max_no_submit_readiness_report_bytes: 4096,
            readiness_report_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_wait_timeout_seconds: 10,
            reference_quote_probe_actor_id: "no-submit-reference-quote-probe".to_string(),
            reference_quote_probe_log_events: true,
            reference_quote_probe_log_commands: true,
            egress_identity_observed_path: None,
            egress_identity_observed_max_bytes: None,
            approved_egress_identity_sha256: None,
            proof_policy: None,
            operator_evidence: Some(valid_operator_evidence()),
        },
    );

    let error = check_bolt_v3_live_canary_gate(&loaded)
        .await
        .expect_err("missing no-submit readiness report must fail closed");

    assert!(
        matches!(error, BoltV3LiveCanaryGateError::ReadinessReportRead { .. }),
        "expected read rejection, got {error:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn live_canary_gate_rejects_malformed_no_submit_report_json() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let report_path = tempdir.path().join("no-submit-readiness.json");
    std::fs::write(&report_path, format!(r#"{{"{STAGES_KEY}":["#))
        .expect("report fixture should be written");
    let loaded = loaded_with_live_canary(
        loaded,
        LiveCanaryBlock {
            approval_id: "operator-approved-canary-001".to_string(),
            no_submit_readiness_report_path: report_path.to_string_lossy().to_string(),
            max_live_order_count: 1,
            max_notional_per_order: "1.00".to_string(),
            max_no_submit_readiness_report_bytes: 4096,
            readiness_report_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_wait_timeout_seconds: 10,
            reference_quote_probe_actor_id: "no-submit-reference-quote-probe".to_string(),
            reference_quote_probe_log_events: true,
            reference_quote_probe_log_commands: true,
            egress_identity_observed_path: None,
            egress_identity_observed_max_bytes: None,
            approved_egress_identity_sha256: None,
            proof_policy: None,
            operator_evidence: Some(valid_operator_evidence()),
        },
    );

    let error = check_bolt_v3_live_canary_gate(&loaded)
        .await
        .expect_err("malformed no-submit readiness report must fail closed");

    assert!(
        matches!(
            error,
            BoltV3LiveCanaryGateError::ReadinessReportParse { .. }
        ),
        "expected parse rejection, got {error:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn live_canary_gate_accepts_report_exactly_at_configured_byte_cap() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let report_path = tempdir.path().join("no-submit-readiness.json");
    write_no_submit_report(&report_path, &[]);
    let report_len = std::fs::metadata(&report_path)
        .expect("report metadata should be readable")
        .len();
    let mut operator_evidence = valid_operator_evidence();
    // Seal the genuine readiness-report hash into the envelope: the report
    // binding is mandatory on the production (proof-disabled) path too. Sealing
    // rewrites only the envelope + consumption proof, not the report file, so
    // the captured report_len byte cap still matches the on-disk report exactly.
    seal_no_submit_readiness_report_into_production_evidence(&mut operator_evidence, &report_path);
    let loaded = loaded_with_live_canary(
        loaded,
        LiveCanaryBlock {
            approval_id: "operator-approved-canary-001".to_string(),
            no_submit_readiness_report_path: report_path.to_string_lossy().to_string(),
            max_live_order_count: 1,
            max_notional_per_order: "1.00".to_string(),
            max_no_submit_readiness_report_bytes: report_len,
            readiness_report_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_wait_timeout_seconds: 10,
            reference_quote_probe_actor_id: "no-submit-reference-quote-probe".to_string(),
            reference_quote_probe_log_events: true,
            reference_quote_probe_log_commands: true,
            egress_identity_observed_path: None,
            egress_identity_observed_max_bytes: None,
            approved_egress_identity_sha256: None,
            proof_policy: None,
            operator_evidence: Some(operator_evidence),
        },
    );

    check_bolt_v3_live_canary_gate(&loaded)
        .await
        .expect("report exactly at configured byte cap should pass");
}

#[tokio::test(flavor = "current_thread")]
async fn live_canary_gate_rejects_no_submit_report_above_configured_byte_cap() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let report_path = tempdir.path().join("no-submit-readiness.json");
    write_report_value(&report_path, serde_json::json!({ STAGES_KEY: [] }));
    let loaded = loaded_with_live_canary(
        loaded,
        LiveCanaryBlock {
            approval_id: "operator-approved-canary-001".to_string(),
            no_submit_readiness_report_path: report_path.to_string_lossy().to_string(),
            max_live_order_count: 1,
            max_notional_per_order: "1.00".to_string(),
            max_no_submit_readiness_report_bytes: 1,
            readiness_report_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_wait_timeout_seconds: 10,
            reference_quote_probe_actor_id: "no-submit-reference-quote-probe".to_string(),
            reference_quote_probe_log_events: true,
            reference_quote_probe_log_commands: true,
            egress_identity_observed_path: None,
            egress_identity_observed_max_bytes: None,
            approved_egress_identity_sha256: None,
            proof_policy: None,
            operator_evidence: Some(valid_operator_evidence()),
        },
    );

    let error = check_bolt_v3_live_canary_gate(&loaded)
        .await
        .expect_err("oversized no-submit readiness report must fail closed");

    assert!(
        matches!(
            error,
            BoltV3LiveCanaryGateError::ReadinessReportTooLarge { .. }
        ),
        "expected size-cap rejection, got {error:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn live_canary_gate_distinguishes_non_object_report_from_missing_stages() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let report_path = tempdir.path().join("no-submit-readiness.json");
    std::fs::write(&report_path, format!(r#"["{STATUS_SATISFIED}"]"#))
        .expect("report fixture should be written");
    let loaded = loaded_with_live_canary(
        loaded,
        LiveCanaryBlock {
            approval_id: "operator-approved-canary-001".to_string(),
            no_submit_readiness_report_path: report_path.to_string_lossy().to_string(),
            max_live_order_count: 1,
            max_notional_per_order: "1.00".to_string(),
            max_no_submit_readiness_report_bytes: 4096,
            readiness_report_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_wait_timeout_seconds: 10,
            reference_quote_probe_actor_id: "no-submit-reference-quote-probe".to_string(),
            reference_quote_probe_log_events: true,
            reference_quote_probe_log_commands: true,
            egress_identity_observed_path: None,
            egress_identity_observed_max_bytes: None,
            approved_egress_identity_sha256: None,
            proof_policy: None,
            operator_evidence: Some(valid_operator_evidence()),
        },
    );

    let error = check_bolt_v3_live_canary_gate(&loaded)
        .await
        .expect_err("non-object no-submit readiness report must fail closed");

    match error {
        BoltV3LiveCanaryGateError::ReadinessReportSchemaVersionMismatch {
            expected,
            actual,
            ..
        } => {
            assert_eq!(expected, NO_SUBMIT_READINESS_SCHEMA_VERSION);
            assert!(
                actual.is_none(),
                "expected actual=None for non-object report, got {actual:?}"
            );
        }
        other => panic!("expected schema-version mismatch rejection, got {other:?}"),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn live_canary_gate_distinguishes_non_array_stages_from_missing_stages() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let report_path = tempdir.path().join("no-submit-readiness.json");
    write_linked_report_value(
        &report_path,
        serde_json::json!({ STAGES_KEY: STATUS_SATISFIED }),
    );
    let loaded = loaded_with_live_canary(
        loaded,
        LiveCanaryBlock {
            approval_id: "operator-approved-canary-001".to_string(),
            no_submit_readiness_report_path: report_path.to_string_lossy().to_string(),
            max_live_order_count: 1,
            max_notional_per_order: "1.00".to_string(),
            max_no_submit_readiness_report_bytes: 4096,
            readiness_report_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_wait_timeout_seconds: 10,
            reference_quote_probe_actor_id: "no-submit-reference-quote-probe".to_string(),
            reference_quote_probe_log_events: true,
            reference_quote_probe_log_commands: true,
            egress_identity_observed_path: None,
            egress_identity_observed_max_bytes: None,
            approved_egress_identity_sha256: None,
            proof_policy: None,
            operator_evidence: Some(valid_operator_evidence()),
        },
    );

    let error = check_bolt_v3_live_canary_gate(&loaded)
        .await
        .expect_err("non-array stages field must fail closed");

    match error {
        BoltV3LiveCanaryGateError::UnsatisfiedNoSubmitReadinessReport { reasons, .. } => {
            assert!(
                reasons
                    .iter()
                    .any(|reason| reason.contains("stages must be an array")),
                "error should name the malformed stages field, got {reasons:?}"
            );
        }
        other => panic!("expected unsatisfied report rejection, got {other:?}"),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn live_canary_gate_rejects_name_only_stage_field() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let report_path = tempdir.path().join("no-submit-readiness.json");
    write_no_submit_report_with_stage_field(&report_path, "name", &[("disconnect", "failed")]);
    let loaded = loaded_with_live_canary(
        loaded,
        LiveCanaryBlock {
            approval_id: "operator-approved-canary-001".to_string(),
            no_submit_readiness_report_path: report_path.to_string_lossy().to_string(),
            max_live_order_count: 1,
            max_notional_per_order: "1.00".to_string(),
            max_no_submit_readiness_report_bytes: 4096,
            readiness_report_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_wait_timeout_seconds: 10,
            reference_quote_probe_actor_id: "no-submit-reference-quote-probe".to_string(),
            reference_quote_probe_log_events: true,
            reference_quote_probe_log_commands: true,
            egress_identity_observed_path: None,
            egress_identity_observed_max_bytes: None,
            approved_egress_identity_sha256: None,
            proof_policy: None,
            operator_evidence: Some(valid_operator_evidence()),
        },
    );

    let error = check_bolt_v3_live_canary_gate(&loaded)
        .await
        .expect_err("report using stale name field must fail closed");

    match error {
        BoltV3LiveCanaryGateError::UnsatisfiedNoSubmitReadinessReport { reasons, .. } => {
            assert!(
                reasons.iter().any(|reason| reason.contains("<unnamed>")),
                "error should reject stale name-only stage as unnamed, got {reasons:?}"
            );
            assert!(
                reasons
                    .iter()
                    .any(|reason| reason.contains("required stage `controlled_disconnect`")),
                "error should treat stale name-only field as missing canonical stage, got {reasons:?}"
            );
            assert!(
                !reasons
                    .iter()
                    .any(|reason| reason.contains("disconnect` status")),
                "stale name-only field must not be accepted as a stage name, got {reasons:?}"
            );
        }
        other => panic!("expected unsatisfied report rejection, got {other:?}"),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn live_canary_gate_accepts_case_insensitive_satisfied_status() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let report_path = tempdir.path().join("no-submit-readiness.json");
    write_no_submit_report_with_stage_field(
        &report_path,
        STAGE_KEY,
        &[(CONTROLLED_CONNECT_STAGE, "SATISFIED")],
    );
    let mut operator_evidence = valid_operator_evidence();
    // Seal the genuine readiness-report hash into the envelope: the report
    // binding is mandatory on the production (proof-disabled) path too.
    seal_no_submit_readiness_report_into_production_evidence(&mut operator_evidence, &report_path);
    let loaded = loaded_with_live_canary(
        loaded,
        LiveCanaryBlock {
            approval_id: "operator-approved-canary-001".to_string(),
            no_submit_readiness_report_path: report_path.to_string_lossy().to_string(),
            max_live_order_count: 1,
            max_notional_per_order: "1.00".to_string(),
            max_no_submit_readiness_report_bytes: 4096,
            readiness_report_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_wait_timeout_seconds: 10,
            reference_quote_probe_actor_id: "no-submit-reference-quote-probe".to_string(),
            reference_quote_probe_log_events: true,
            reference_quote_probe_log_commands: true,
            egress_identity_observed_path: None,
            egress_identity_observed_max_bytes: None,
            approved_egress_identity_sha256: None,
            proof_policy: None,
            operator_evidence: Some(operator_evidence),
        },
    );

    let report = check_bolt_v3_live_canary_gate(&loaded)
        .await
        .expect("uppercase satisfied status should pass");

    assert_eq!(report.approval_id(), "operator-approved-canary-001");
}

fn loaded_with_live_canary(
    loaded: LoadedBoltV3Config,
    live_canary: LiveCanaryBlock,
) -> LoadedBoltV3Config {
    let mut root = loaded.root;
    root.live_canary = Some(live_canary);
    LoadedBoltV3Config { root, ..loaded }
}

fn loaded_without_live_canary(loaded: LoadedBoltV3Config) -> LoadedBoltV3Config {
    let mut root = loaded.root;
    root.live_canary = None;
    LoadedBoltV3Config { root, ..loaded }
}

fn valid_operator_evidence() -> bolt_v2::bolt_v3_config::LiveCanaryOperatorEvidenceBlock {
    support::valid_live_canary_operator_evidence()
}

fn valid_operator_evidence_for_window(
    approval_not_before_unix_seconds: i64,
    approval_not_after_unix_seconds: i64,
) -> LiveCanaryOperatorEvidenceBlock {
    let mut evidence = valid_operator_evidence();
    evidence.approval_not_before_unix_seconds = approval_not_before_unix_seconds;
    evidence.approval_not_after_unix_seconds = approval_not_after_unix_seconds;
    let envelope = valid_approval_envelope_value(&evidence);
    bind_approval_envelope_value(&mut evidence, envelope);
    evidence
}

async fn check_operator_evidence_rejection(
    operator_evidence: LiveCanaryOperatorEvidenceBlock,
    expected_field: &str,
) -> BoltV3LiveCanaryGateError {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let loaded = loaded_with_live_canary(
        loaded,
        LiveCanaryBlock {
            approval_id: "operator-approved-canary-001".to_string(),
            no_submit_readiness_report_path: "not-read-before-operator-evidence-shape-check.json"
                .to_string(),
            max_live_order_count: 1,
            max_notional_per_order: "1.00".to_string(),
            max_no_submit_readiness_report_bytes: 4096,
            readiness_report_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_max_age_seconds: TEST_READINESS_REPORT_MAX_AGE_SECONDS,
            reference_quote_wait_timeout_seconds: 10,
            reference_quote_probe_actor_id: "no-submit-reference-quote-probe".to_string(),
            reference_quote_probe_log_events: true,
            reference_quote_probe_log_commands: true,
            egress_identity_observed_path: None,
            egress_identity_observed_max_bytes: None,
            approved_egress_identity_sha256: None,
            proof_policy: None,
            operator_evidence: Some(operator_evidence),
        },
    );

    match check_bolt_v3_live_canary_gate(&loaded).await {
        Ok(report) => panic!("{expected_field} must fail closed when blank, got {report:?}"),
        Err(error) => error,
    }
}

fn blank_operator_evidence_value() -> String {
    " \t\n".to_string()
}

fn write_no_submit_report(path: &std::path::Path, stages: &[(&str, &str)]) {
    write_no_submit_report_with_stage_field(path, STAGE_KEY, stages);
}

fn write_no_submit_report_at(
    path: &std::path::Path,
    stages: &[(&str, &str)],
    generated_at_unix_seconds: u64,
) {
    let stages = complete_stage_values_with_overrides(stages, STAGE_KEY);
    let mut report = linked_report_object(stages);
    report.insert(
        GENERATED_AT_UNIX_SECONDS_KEY.to_string(),
        serde_json::json!(generated_at_unix_seconds),
    );
    write_report_value(path, serde_json::Value::Object(report));
}

fn write_no_submit_report_with_linkage(
    path: &std::path::Path,
    approval_id_hash: &str,
    executable_identity: &str,
    config_bundle_checksum: &str,
) {
    let stages = [
        OPERATOR_APPROVAL_STAGE,
        SECRET_RESOLUTION_STAGE,
        LIVE_NODE_BUILD_STAGE,
        CONTROLLED_CONNECT_STAGE,
        REFERENCE_READINESS_STAGE,
        CONTROLLED_DISCONNECT_STAGE,
        REPORT_WRITE_STAGE,
    ]
    .into_iter()
    .map(|stage| serde_json::json!({ STAGE_KEY: stage, STATUS_KEY: STATUS_SATISFIED }))
    .collect::<Vec<_>>();
    write_report_value(
        path,
        serde_json::json!({
            SCHEMA_VERSION_KEY: NO_SUBMIT_READINESS_SCHEMA_VERSION,
            APPROVAL_ID_HASH_KEY: approval_id_hash,
            EXECUTABLE_IDENTITY_KEY: executable_identity,
            CONFIG_BUNDLE_CHECKSUM_KEY: config_bundle_checksum,
            GENERATED_AT_UNIX_SECONDS_KEY: current_unix_seconds_for_test(),
            STAGES_KEY: stages,
        }),
    );
}

fn write_report_value(path: &std::path::Path, value: serde_json::Value) {
    std::fs::write(
        path,
        serde_json::to_string_pretty(&value).expect("report fixture should serialize"),
    )
    .expect("report fixture should be written");
}

fn write_linked_report_value(path: &std::path::Path, value: serde_json::Value) {
    let mut object = value
        .as_object()
        .expect("linked report fixture must be an object")
        .clone();
    let mut linkage = linked_report_object(Vec::new());
    linkage.remove(STAGES_KEY);
    for (key, value) in linkage {
        object.entry(key).or_insert(value);
    }
    write_report_value(path, serde_json::Value::Object(object));
}

fn write_approval_consumption_proof_with_override(
    evidence: &LiveCanaryOperatorEvidenceBlock,
    field: &'static str,
    value: serde_json::Value,
) {
    let mut proof = approval_consumption_proof(evidence);
    proof
        .as_object_mut()
        .expect("approval consumption proof should be an object")
        .insert(field.to_string(), value);
    write_report_value(
        std::path::Path::new(&evidence.approval_consumption_path),
        proof,
    );
}

fn write_approval_consumption_proof_without_field(
    evidence: &LiveCanaryOperatorEvidenceBlock,
    field: &'static str,
) {
    let mut proof = approval_consumption_proof(evidence);
    proof
        .as_object_mut()
        .expect("approval consumption proof should be an object")
        .remove(field);
    write_report_value(
        std::path::Path::new(&evidence.approval_consumption_path),
        proof,
    );
}

fn write_valid_approval_consumption_proof(evidence: &LiveCanaryOperatorEvidenceBlock) {
    write_report_value(
        std::path::Path::new(&evidence.approval_consumption_path),
        approval_consumption_proof(evidence),
    );
}

fn bind_approval_envelope_value(
    evidence: &mut LiveCanaryOperatorEvidenceBlock,
    value: serde_json::Value,
) {
    let bytes = serde_json::to_vec_pretty(&value).expect("approval envelope should serialize");
    std::fs::write(&evidence.approval_envelope_path, &bytes)
        .expect("approval envelope should be written");
    evidence.approval_envelope_sha256 = sha256_hex(&bytes);
    write_valid_approval_consumption_proof(evidence);
}

fn make_strategy_input_reference_quote_stale(
    evidence: &mut LiveCanaryOperatorEvidenceBlock,
    stale_by_seconds: u64,
) {
    let path = std::path::Path::new(&evidence.strategy_input_evidence_path);
    let mut value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(path).expect("strategy input should read"))
            .expect("strategy input should parse");
    let one_second_ms: u64 = std::time::Duration::from_secs(1)
        .as_millis()
        .try_into()
        .expect("one second should fit in u64 milliseconds");
    let stale_reference_quote_ts_ms = current_unix_seconds_for_test()
        .saturating_sub(stale_by_seconds)
        .saturating_mul(one_second_ms);
    value
        .as_object_mut()
        .expect("strategy input should be an object")
        .insert(
            "reference_quote_ts_event".to_string(),
            serde_json::json!(stale_reference_quote_ts_ms),
        );
    let bytes = serde_json::to_vec(&value).expect("strategy input should serialize");
    std::fs::write(path, &bytes).expect("strategy input should rewrite");
    evidence.strategy_input_evidence_sha256 = sha256_hex(&bytes);
    let envelope = valid_approval_envelope_value(evidence);
    bind_approval_envelope_value(evidence, envelope);
}

fn make_latest_decision_evidence_notional(
    evidence: &LiveCanaryOperatorEvidenceBlock,
    notional: &str,
) {
    let path = std::path::Path::new(&evidence.decision_evidence_path);
    let text = std::fs::read_to_string(path).expect("decision evidence should read");
    let mut lines = Vec::new();
    for line in text.lines() {
        let mut value: serde_json::Value =
            serde_json::from_str(line).expect("decision evidence line should parse");
        if value.get("kind").and_then(serde_json::Value::as_str) == Some("admission_decision") {
            value
                .get_mut("decision")
                .and_then(serde_json::Value::as_object_mut)
                .expect("admission decision should be an object")
                .insert("notional".to_string(), serde_json::json!(notional));
        }
        lines.push(serde_json::to_string(&value).expect("decision evidence should serialize"));
    }
    let mut rewritten = lines.join("\n");
    rewritten.push('\n');
    std::fs::write(path, rewritten).expect("decision evidence should rewrite");
}

fn valid_approval_envelope_value(evidence: &LiveCanaryOperatorEvidenceBlock) -> serde_json::Value {
    let mut envelope = serde_json::json!({
        "schema_version": 1,
        "record_kind": "phase8_operator_approval_envelope",
        "head_sha": evidence.head_sha,
        "ssm_manifest_sha256": evidence.ssm_manifest_sha256,
        "strategy_input_evidence_sha256": evidence.strategy_input_evidence_sha256,
        "financial_envelope_sha256": evidence.financial_envelope_sha256,
        "pre_run_state_sha256": evidence.pre_run_state_sha256,
        "abort_plan_sha256": evidence.abort_plan_sha256,
        "approval_id_hash": sha256_hex("operator-approved-canary-001".as_bytes()),
        "approval_nonce_sha256": evidence.approval_nonce_sha256,
        "approval_not_before_unix_secs": evidence.approval_not_before_unix_seconds,
        "approval_not_after_unix_secs": evidence.approval_not_after_unix_seconds,
        "canary_evidence_path_hash": sha256_hex(evidence.canary_evidence_path.as_bytes()),
    });
    // Seal the operator-approved gate-session and canary proof order-intent
    // file-content hashes into the envelope whenever the TOML evidence binds
    // them, mirroring the production producer. This exercises the gate's
    // envelope-binding check (the envelope, not just the self-declared TOML
    // hash, must match the live file content).
    let envelope_object = envelope
        .as_object_mut()
        .expect("approval envelope should be an object");
    if let Some(expected_gate_session_sha256) = &evidence.expected_gate_session_sha256 {
        envelope_object.insert(
            "expected_gate_session_sha256".to_string(),
            serde_json::json!(expected_gate_session_sha256),
        );
    }
    if let Some(canary_proof_order_intent_sha256) = &evidence.canary_proof_order_intent_sha256 {
        envelope_object.insert(
            "canary_proof_order_intent_sha256".to_string(),
            serde_json::json!(canary_proof_order_intent_sha256),
        );
    }
    if let Some(no_submit_readiness_report_sha256) = &evidence.no_submit_readiness_report_sha256 {
        envelope_object.insert(
            "no_submit_readiness_report_sha256".to_string(),
            serde_json::json!(no_submit_readiness_report_sha256),
        );
    }
    if let Some(strategy_cancel_path) = &evidence.strategy_cancel_path {
        envelope
            .as_object_mut()
            .expect("approval envelope should be an object")
            .insert(
                "strategy_cancel_path_hash".to_string(),
                serde_json::json!(sha256_hex(strategy_cancel_path.as_bytes())),
            );
    }
    envelope
}

fn approval_consumption_proof(evidence: &LiveCanaryOperatorEvidenceBlock) -> serde_json::Value {
    let mut proof = serde_json::json!({
        "schema_version": APPROVAL_CONSUMPTION_SCHEMA_VERSION,
        "record_kind": APPROVAL_CONSUMPTION_RECORD_KIND,
        "head_sha": evidence.head_sha,
        "root_toml_sha256": root_toml_sha256_for_test(),
        "approval_envelope_sha256": evidence.approval_envelope_sha256,
        "ssm_manifest_sha256": evidence.ssm_manifest_sha256,
        "strategy_input_evidence_sha256": evidence.strategy_input_evidence_sha256,
        "financial_envelope_sha256": evidence.financial_envelope_sha256,
        "pre_run_state_sha256": evidence.pre_run_state_sha256,
        "abort_plan_sha256": evidence.abort_plan_sha256,
        "approval_id_hash": sha256_hex("operator-approved-canary-001".as_bytes()),
        "approval_nonce_sha256": evidence.approval_nonce_sha256,
        "approval_not_before_unix_secs": evidence.approval_not_before_unix_seconds,
        "approval_not_after_unix_secs": evidence.approval_not_after_unix_seconds,
        "canary_evidence_path_hash": sha256_hex(evidence.canary_evidence_path.as_bytes()),
        "consumed_unix_secs": current_unix_seconds_for_test() as i64,
    });
    if let Some(strategy_cancel_path) = &evidence.strategy_cancel_path {
        proof
            .as_object_mut()
            .expect("approval consumption proof should be an object")
            .insert(
                "strategy_cancel_path_hash".to_string(),
                serde_json::json!(sha256_hex(strategy_cancel_path.as_bytes())),
            );
    }
    proof
}

fn linked_report_object(
    stages: Vec<serde_json::Value>,
) -> serde_json::Map<String, serde_json::Value> {
    let mut object = serde_json::Map::new();
    object.insert(
        SCHEMA_VERSION_KEY.to_string(),
        serde_json::json!(NO_SUBMIT_READINESS_SCHEMA_VERSION),
    );
    object.insert(
        APPROVAL_ID_HASH_KEY.to_string(),
        serde_json::json!(sha256_hex("operator-approved-canary-001".as_bytes())),
    );
    object.insert(
        EXECUTABLE_IDENTITY_KEY.to_string(),
        serde_json::json!(current_executable_identity()),
    );
    object.insert(
        CONFIG_BUNDLE_CHECKSUM_KEY.to_string(),
        serde_json::json!(
            load_bolt_v3_config(&support::repo_path("tests/fixtures/bolt_v3/root.toml"))
                .expect("fixture v3 config should load")
                .config_bundle_checksum
        ),
    );
    object.insert(
        GENERATED_AT_UNIX_SECONDS_KEY.to_string(),
        serde_json::json!(current_unix_seconds_for_test()),
    );
    object.insert(STAGES_KEY.to_string(), serde_json::Value::Array(stages));
    object
}

fn complete_stage_values() -> Vec<serde_json::Value> {
    [
        OPERATOR_APPROVAL_STAGE,
        SECRET_RESOLUTION_STAGE,
        LIVE_NODE_BUILD_STAGE,
        CONTROLLED_CONNECT_STAGE,
        REFERENCE_READINESS_STAGE,
        CONTROLLED_DISCONNECT_STAGE,
        REPORT_WRITE_STAGE,
    ]
    .into_iter()
    .map(|stage| serde_json::json!({ STAGE_KEY: stage, STATUS_KEY: STATUS_SATISFIED }))
    .collect()
}

fn complete_stage_values_with_overrides(
    overrides: &[(&str, &str)],
    stage_field: &str,
) -> Vec<serde_json::Value> {
    let mut complete_stages = [
        OPERATOR_APPROVAL_STAGE,
        SECRET_RESOLUTION_STAGE,
        LIVE_NODE_BUILD_STAGE,
        CONTROLLED_CONNECT_STAGE,
        REFERENCE_READINESS_STAGE,
        CONTROLLED_DISCONNECT_STAGE,
        REPORT_WRITE_STAGE,
    ]
    .into_iter()
    .map(|stage| (stage, STATUS_SATISFIED))
    .collect::<Vec<_>>();
    for &(stage, status) in overrides {
        if let Some(existing) = complete_stages
            .iter_mut()
            .find(|(existing_stage, _)| *existing_stage == stage)
        {
            existing.1 = status;
        } else {
            complete_stages.push((stage, status));
        }
    }
    complete_stages
        .iter()
        .map(|(stage, status)| serde_json::json!({ stage_field: stage, STATUS_KEY: status }))
        .collect()
}

fn write_no_submit_report_with_stage_field(
    path: &std::path::Path,
    stage_field: &str,
    stages: &[(&str, &str)],
) {
    let stages = complete_stage_values_with_overrides(stages, stage_field);
    write_linked_report_value(path, serde_json::json!({ STAGES_KEY: stages }));
}

fn current_executable_identity() -> String {
    let path = std::env::current_exe().expect("current test executable path should resolve");
    sha256_hex(&std::fs::read(path).expect("current test executable should be readable"))
}

fn current_unix_seconds_for_test() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("test system clock should be after UNIX_EPOCH")
        .as_secs()
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn root_toml_sha256_for_test() -> String {
    sha256_hex(
        &std::fs::read(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
            .expect("fixture root TOML should be readable"),
    )
}
