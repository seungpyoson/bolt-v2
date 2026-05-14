mod support;

use bolt_v2::{
    bolt_v3_config::{LiveCanaryBlock, LoadedBoltV3Config, load_bolt_v3_config},
    bolt_v3_tiny_canary_evidence::{
        Phase8CanaryBlockReason, Phase8CanaryEvidence, Phase8CanaryOutcome,
        Phase8CanaryPreflightStatus, Phase8EvidenceRef, Phase8LiveCanaryResultRefs,
        Phase8LiveOrderRef, Phase8OperatorApprovalEnvelope, Phase8StrategyInputSafetyAudit,
        evaluate_phase8_canary_preflight,
    },
};
use rust_decimal::Decimal;
use serde_json::Value;

#[tokio::test]
async fn preflight_blocks_missing_phase7_report_before_build() {
    let loaded = loaded_with_live_canary("reports/missing-no-submit-readiness.json");
    let audit = Phase8StrategyInputSafetyAudit::approved();

    let report = evaluate_phase8_canary_preflight(
        &loaded,
        "7f2d981f584a0378842d9a76fffd9cd03fce2ce5",
        audit,
    )
    .await;

    assert_eq!(
        report.no_submit_report_status,
        Phase8CanaryPreflightStatus::Missing
    );
    assert!(
        report
            .block_reasons
            .contains(&Phase8CanaryBlockReason::MissingNoSubmitReadinessReport)
    );
    assert!(!report.can_enter_live_runner());
}

#[tokio::test]
async fn preflight_blocks_strategy_input_safety_audit_before_build() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let report_path = temp.path().join("no-submit-readiness.json");
    write_satisfied_no_submit_readiness_report(&report_path);
    let loaded = loaded_with_live_canary(report_path.to_str().expect("utf8 report path"));

    let report = evaluate_phase8_canary_preflight(
        &loaded,
        "7f2d981f584a0378842d9a76fffd9cd03fce2ce5",
        Phase8StrategyInputSafetyAudit::blocked(vec![
            Phase8CanaryBlockReason::StrategyInputSafetyAuditBlocked,
        ]),
    )
    .await;

    assert_eq!(
        report.no_submit_report_status,
        Phase8CanaryPreflightStatus::AcceptedByGate
    );
    assert!(
        report
            .block_reasons
            .contains(&Phase8CanaryBlockReason::StrategyInputSafetyAuditBlocked)
    );
    assert!(!report.can_enter_live_runner());
}

#[tokio::test]
async fn preflight_blocks_live_order_count_above_one_before_build() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let report_path = temp.path().join("no-submit-readiness.json");
    write_satisfied_no_submit_readiness_report(&report_path);
    let mut loaded = loaded_with_live_canary(report_path.to_str().expect("utf8 report path"));
    loaded
        .root
        .live_canary
        .as_mut()
        .expect("live canary should exist")
        .max_live_order_count = 2;

    let report = evaluate_phase8_canary_preflight(
        &loaded,
        "7f2d981f584a0378842d9a76fffd9cd03fce2ce5",
        Phase8StrategyInputSafetyAudit::approved(),
    )
    .await;

    assert!(
        report
            .block_reasons
            .contains(&Phase8CanaryBlockReason::LiveOrderCountCapNotOne)
    );
    assert_eq!(report.max_live_order_count, Some(2));
    assert!(!report.can_enter_live_runner());
}

#[test]
fn strategy_audit_blocks_non_positive_realized_volatility() {
    let audit = Phase8StrategyInputSafetyAudit::from_strategy_inputs(Decimal::ZERO, 300);

    assert!(
        audit
            .block_reasons()
            .contains(&Phase8CanaryBlockReason::NonPositiveRealizedVolatility)
    );
    assert!(!audit.is_approved());
}

#[test]
fn strategy_audit_blocks_zero_time_to_expiry() {
    let audit = Phase8StrategyInputSafetyAudit::from_strategy_inputs(Decimal::new(25, 1), 0);

    assert!(
        audit
            .block_reasons()
            .contains(&Phase8CanaryBlockReason::NonPositiveTimeToExpiry)
    );
    assert!(!audit.is_approved());
}

#[test]
fn strategy_audit_verifies_input_evidence_hash_before_approving() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let evidence_path = temp.path().join("strategy-input-evidence.json");
    std::fs::write(
        &evidence_path,
        r#"{"realized_volatility":"2.5","seconds_to_expiry":300}"#,
    )
    .expect("strategy input evidence should write");
    let evidence_hash = Phase8OperatorApprovalEnvelope::sha256_file(&evidence_path)
        .expect("strategy input evidence hash should compute");

    let audit = Phase8StrategyInputSafetyAudit::from_evidence_file(&evidence_path, &evidence_hash)
        .expect("matching strategy input evidence should parse");

    assert!(audit.is_approved());
}

#[test]
fn strategy_audit_rejects_input_evidence_hash_mismatch() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let evidence_path = temp.path().join("strategy-input-evidence.json");
    std::fs::write(
        &evidence_path,
        r#"{"realized_volatility":"2.5","seconds_to_expiry":300}"#,
    )
    .expect("strategy input evidence should write");

    let error = Phase8StrategyInputSafetyAudit::from_evidence_file(&evidence_path, "wrong-hash")
        .expect_err("mismatched strategy input evidence should fail");

    assert!(
        error.to_string().contains("strategy input evidence sha256"),
        "error should mention strategy input evidence hash mismatch: {error}"
    );
}

#[test]
fn dry_canary_evidence_serializes_join_keys_without_raw_approval_id() {
    let evidence = Phase8CanaryEvidence::dry_no_submit_proof(
        evidence_input(),
        Phase8EvidenceRef {
            path_hash: "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
                .to_string(),
            record_hash: "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"
                .to_string(),
        },
    );

    let value = serde_json::to_value(&evidence).expect("evidence should serialize");
    assert_eq!(
        value["outcome"],
        Value::String("dry_no_submit_proof".to_string())
    );
    assert_eq!(value["max_live_order_count"], Value::from(1));
    assert_eq!(
        value["max_notional_per_order"],
        Value::String("0.25".to_string())
    );
    assert_ne!(
        value["approval_id_hash"],
        Value::String("operator-approved-canary-001".to_string())
    );

    let rendered = serde_json::to_string(&evidence).expect("evidence should render");
    assert!(!rendered.contains("operator-approved-canary-001"));
    assert!(rendered.contains("decision_evidence_ref"));
    assert!(rendered.contains("ssm_manifest_ref"));
    assert!(rendered.contains("strategy_input_evidence_ref"));
    assert!(rendered.contains("submit_admission_ref"));
    assert!(rendered.contains("runtime_capture_ref"));
}

#[test]
fn dry_canary_evidence_writer_creates_redacted_json_file() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let evidence_path = temp.path().join("phase8-canary-evidence.json");
    let evidence = Phase8CanaryEvidence::dry_no_submit_proof(
        evidence_input(),
        Phase8EvidenceRef {
            path_hash: "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
                .to_string(),
            record_hash: "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"
                .to_string(),
        },
    );

    evidence
        .write_json_file(&evidence_path)
        .expect("evidence should write");

    let rendered = std::fs::read_to_string(&evidence_path).expect("evidence should read");
    assert!(!rendered.contains("operator-approved-canary-001"));
    let value: Value = serde_json::from_str(&rendered).expect("evidence should parse");
    assert_eq!(
        value["outcome"],
        Value::String("dry_no_submit_proof".to_string())
    );
}

#[test]
fn dry_canary_evidence_writer_rejects_existing_json_file() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let evidence_path = temp.path().join("phase8-canary-evidence.json");
    let evidence = Phase8CanaryEvidence::dry_no_submit_proof(
        evidence_input(),
        Phase8EvidenceRef {
            path_hash: "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
                .to_string(),
            record_hash: "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"
                .to_string(),
        },
    );

    evidence
        .write_json_file(&evidence_path)
        .expect("evidence should write");
    let original = std::fs::read_to_string(&evidence_path).expect("evidence should read");

    let replacement = Phase8CanaryEvidence::blocked_before_submit(
        evidence_input(),
        Phase8CanaryBlockReason::RootConfigHashUnavailable,
    );
    let error = replacement
        .write_json_file(&evidence_path)
        .expect_err("existing evidence must not be overwritten");

    assert!(
        error.to_string().contains("already exists"),
        "error should explain existing evidence: {error}"
    );
    let rendered = std::fs::read_to_string(&evidence_path).expect("evidence should read");
    assert_eq!(rendered, original);
    assert!(!rendered.contains("blocked_before_submit"));
}

#[test]
fn decision_evidence_unavailable_blocks_before_submit_admission() {
    let evidence = Phase8CanaryEvidence::blocked_before_submit(
        evidence_input(),
        Phase8CanaryBlockReason::DecisionEvidenceUnavailable,
    );

    assert_eq!(evidence.outcome, Phase8CanaryOutcome::BlockedBeforeSubmit);
    assert_eq!(evidence.submit_admission_ref.admitted_order_count, 0);
    assert!(
        evidence
            .block_reasons
            .contains(&Phase8CanaryBlockReason::DecisionEvidenceUnavailable)
    );
    assert!(evidence.decision_evidence_ref.is_none());
    assert!(evidence.nt_lifecycle_refs.is_empty());
}

#[test]
fn live_canary_evidence_requires_submit_cancel_and_restart_refs_without_raw_ids() {
    let evidence = Phase8CanaryEvidence::live_canary_proof(
        evidence_input(),
        Phase8EvidenceRef {
            path_hash: "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
                .to_string(),
            record_hash: "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"
                .to_string(),
        },
        Phase8LiveOrderRef {
            client_order_id_hash:
                "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee".to_string(),
            venue_order_id_hash: "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"
                .to_string(),
        },
        Phase8LiveCanaryResultRefs {
            nt_submit_event_ref: Phase8EvidenceRef {
                path_hash: "1111111111111111111111111111111111111111111111111111111111111111"
                    .to_string(),
                record_hash: "2222222222222222222222222222222222222222222222222222222222222222"
                    .to_string(),
            },
            venue_order_state_ref: Phase8EvidenceRef {
                path_hash: "3333333333333333333333333333333333333333333333333333333333333333"
                    .to_string(),
                record_hash: "4444444444444444444444444444444444444444444444444444444444444444"
                    .to_string(),
            },
            strategy_cancel_ref: Some(Phase8EvidenceRef {
                path_hash: "5555555555555555555555555555555555555555555555555555555555555555"
                    .to_string(),
                record_hash: "6666666666666666666666666666666666666666666666666666666666666666"
                    .to_string(),
            }),
            restart_reconciliation_ref: Phase8EvidenceRef {
                path_hash: "7777777777777777777777777777777777777777777777777777777777777777"
                    .to_string(),
                record_hash: "8888888888888888888888888888888888888888888888888888888888888888"
                    .to_string(),
            },
        },
    );

    assert_eq!(evidence.outcome, Phase8CanaryOutcome::LiveCanaryProof);
    assert_eq!(evidence.submit_admission_ref.admitted_order_count, 1);
    assert!(evidence.block_reasons.is_empty());
    assert!(evidence.live_order_ref.is_some());
    assert!(evidence.nt_submit_event_ref.is_some());
    assert!(evidence.venue_order_state_ref.is_some());
    assert!(evidence.strategy_cancel_ref.is_some());
    assert!(evidence.restart_reconciliation_ref.is_some());

    let rendered = serde_json::to_string(&evidence).expect("evidence should render");
    assert!(!rendered.contains("operator-approved-canary-001"));
    assert!(!rendered.contains("client-order-001"));
    assert!(rendered.contains("restart_reconciliation_ref"));
}

#[test]
fn operator_approval_envelope_rejects_head_or_checksum_mismatch() {
    let envelope = Phase8OperatorApprovalEnvelope {
        head_sha: "expected-head".to_string(),
        root_toml_path: "config/live.local.toml".to_string(),
        root_toml_sha256: "expected-config-hash".to_string(),
        ssm_manifest_path: "phase8-ssm-manifest.json".to_string(),
        ssm_manifest_sha256: "expected-ssm-hash".to_string(),
        strategy_input_evidence_path: "phase8-strategy-input-evidence.json".to_string(),
        strategy_input_evidence_sha256: "expected-strategy-input-hash".to_string(),
        operator_approval_id: "operator-approved-canary-001".to_string(),
        canary_evidence_path: "phase8-canary-evidence.json".to_string(),
    };

    let error = envelope
        .validate_against(
            "actual-head",
            "actual-config-hash",
            "operator-approved-canary-001",
        )
        .expect_err("mismatched envelope should fail");

    assert!(
        error
            .to_string()
            .contains("phase8 operator approval head_sha does not match current head")
    );
}

#[test]
fn operator_approval_envelope_verifies_ssm_manifest_hash() {
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
    std::fs::write(
        &strategy_input_path,
        r#"{"realized_volatility":"2.5","seconds_to_expiry":300}"#,
    )
    .expect("strategy input evidence should write");
    let strategy_input_hash = Phase8OperatorApprovalEnvelope::sha256_file(&strategy_input_path)
        .expect("strategy input evidence hash should compute");
    let mut envelope = Phase8OperatorApprovalEnvelope {
        head_sha: "expected-head".to_string(),
        root_toml_path: "config/live.local.toml".to_string(),
        root_toml_sha256: "expected-config-hash".to_string(),
        ssm_manifest_path: manifest_path.to_string_lossy().to_string(),
        ssm_manifest_sha256: manifest_hash,
        strategy_input_evidence_path: strategy_input_path.to_string_lossy().to_string(),
        strategy_input_evidence_sha256: strategy_input_hash,
        operator_approval_id: "operator-approved-canary-001".to_string(),
        canary_evidence_path: "phase8-canary-evidence.json".to_string(),
    };

    envelope
        .validate_against(
            "expected-head",
            "expected-config-hash",
            "operator-approved-canary-001",
        )
        .expect("matching manifest hash should pass");

    envelope.ssm_manifest_sha256 = "wrong-ssm-hash".to_string();
    let error = envelope
        .validate_against(
            "expected-head",
            "expected-config-hash",
            "operator-approved-canary-001",
        )
        .expect_err("mismatched manifest hash should fail");

    assert!(
        error.to_string().contains("ssm_manifest_sha256"),
        "error should mention SSM manifest hash mismatch: {error}"
    );
}

#[test]
fn operator_approval_envelope_verifies_strategy_input_evidence_hash() {
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
    std::fs::write(
        &strategy_input_path,
        r#"{"realized_volatility":"2.5","seconds_to_expiry":300}"#,
    )
    .expect("strategy input evidence should write");
    let strategy_input_hash = Phase8OperatorApprovalEnvelope::sha256_file(&strategy_input_path)
        .expect("strategy input evidence hash should compute");
    let mut envelope = Phase8OperatorApprovalEnvelope {
        head_sha: "expected-head".to_string(),
        root_toml_path: "config/live.local.toml".to_string(),
        root_toml_sha256: "expected-config-hash".to_string(),
        ssm_manifest_path: manifest_path.to_string_lossy().to_string(),
        ssm_manifest_sha256: manifest_hash,
        strategy_input_evidence_path: strategy_input_path.to_string_lossy().to_string(),
        strategy_input_evidence_sha256: strategy_input_hash,
        operator_approval_id: "operator-approved-canary-001".to_string(),
        canary_evidence_path: "phase8-canary-evidence.json".to_string(),
    };

    envelope
        .validate_against(
            "expected-head",
            "expected-config-hash",
            "operator-approved-canary-001",
        )
        .expect("matching strategy input evidence hash should pass");

    envelope.strategy_input_evidence_sha256 = "wrong-strategy-input-hash".to_string();
    let error = envelope
        .validate_against(
            "expected-head",
            "expected-config-hash",
            "operator-approved-canary-001",
        )
        .expect_err("mismatched strategy input evidence hash should fail");

    assert!(
        error.to_string().contains("strategy_input_evidence_sha256"),
        "error should mention strategy input evidence hash mismatch: {error}"
    );
}

fn loaded_with_live_canary(report_path: &str) -> LoadedBoltV3Config {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    loaded.root.live_canary = Some(LiveCanaryBlock {
        approval_id: "operator-approved-canary-001".to_string(),
        no_submit_readiness_report_path: report_path.to_string(),
        max_no_submit_readiness_report_bytes: 4096,
        max_live_order_count: 1,
        max_notional_per_order: "0.25".to_string(),
    });
    loaded
}

fn write_satisfied_no_submit_readiness_report(path: &std::path::Path) {
    let json = serde_json::json!({
        "schema_version": 1,
        "head_sha": "7f2d981f584a0378842d9a76fffd9cd03fce2ce5",
        "root_config_sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "operator_approval_id_hash": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        "live_canary_approval_id_hash": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
        "stages": [
            {"stage": "operator_approval", "status": "satisfied"},
            {"stage": "secret_resolution", "status": "satisfied"},
            {"stage": "live_node_build", "status": "satisfied"},
            {"stage": "controlled_connect", "status": "satisfied"},
            {"stage": "reference_readiness", "status": "satisfied"},
            {"stage": "controlled_disconnect", "status": "satisfied"},
            {"stage": "report_write", "status": "satisfied"}
        ],
        "redactions": []
    });
    std::fs::create_dir_all(path.parent().expect("report parent should exist"))
        .expect("report parent should create");
    std::fs::write(
        path,
        serde_json::to_vec(&json).expect("report should serialize"),
    )
    .expect("report should write");
}

fn runtime_capture_ref() -> bolt_v2::bolt_v3_tiny_canary_evidence::Phase8RuntimeCaptureRef {
    bolt_v2::bolt_v3_tiny_canary_evidence::Phase8RuntimeCaptureRef {
        spool_root_hash: "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"
            .to_string(),
        run_id: "phase8-dry-run".to_string(),
    }
}

fn evidence_input() -> bolt_v2::bolt_v3_tiny_canary_evidence::Phase8CanaryEvidenceInput {
    bolt_v2::bolt_v3_tiny_canary_evidence::Phase8CanaryEvidenceInput {
        head_sha: "7f2d981f584a0378842d9a76fffd9cd03fce2ce5".to_string(),
        root_config_sha256: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            .to_string(),
        ssm_manifest_sha256: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
            .to_string(),
        ssm_manifest_ref: Phase8EvidenceRef {
            path_hash: "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"
                .to_string(),
            record_hash: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
                .to_string(),
        },
        strategy_input_evidence_ref: Phase8EvidenceRef {
            path_hash: "9999999999999999999999999999999999999999999999999999999999999999"
                .to_string(),
            record_hash: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                .to_string(),
        },
        approval_id: "operator-approved-canary-001".to_string(),
        max_live_order_count: 1,
        max_notional_per_order: Decimal::new(25, 2),
        runtime_capture_ref: runtime_capture_ref(),
    }
}
