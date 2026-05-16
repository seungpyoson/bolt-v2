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
    let audit = Phase8StrategyInputSafetyAudit::from_strategy_inputs(
        Decimal::ZERO,
        300,
        Decimal::new(100_000, 0),
        Decimal::new(100_000, 0),
        Decimal::new(125, 1),
        Decimal::new(125, 1),
        Decimal::ZERO,
        "chainlink_data_streams",
        1_234_567_890,
    );

    assert!(
        audit
            .block_reasons()
            .contains(&Phase8CanaryBlockReason::NonPositiveRealizedVolatility)
    );
    assert!(!audit.is_approved());
}

#[test]
fn strategy_audit_blocks_zero_time_to_expiry() {
    let audit = Phase8StrategyInputSafetyAudit::from_strategy_inputs(
        Decimal::new(25, 1),
        0,
        Decimal::new(100_000, 0),
        Decimal::new(100_000, 0),
        Decimal::new(125, 1),
        Decimal::new(125, 1),
        Decimal::ZERO,
        "chainlink_data_streams",
        1_234_567_890,
    );

    assert!(
        audit
            .block_reasons()
            .contains(&Phase8CanaryBlockReason::NonPositiveTimeToExpiry)
    );
    assert!(!audit.is_approved());
}

#[test]
fn strategy_audit_blocks_non_positive_spot_or_price_to_beat_evidence() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let strategy_input_path = temp.path().join("phase8-strategy-input-evidence.json");
    std::fs::write(
        &strategy_input_path,
        r#"{"realized_volatility":"2.5","seconds_to_expiry":300,"spot_price":"0","price_to_beat_value":"100000.0","expected_edge_basis_points":"12.5","worst_case_edge_basis_points":"12.5","fee_rate_basis_points":"0","price_to_beat_source":"chainlink_data_streams","reference_quote_ts_event":1234567890}"#,
    )
    .expect("strategy input evidence should write");
    let strategy_input_hash = Phase8OperatorApprovalEnvelope::sha256_file(&strategy_input_path)
        .expect("strategy input evidence hash should compute");

    let audit = Phase8StrategyInputSafetyAudit::from_evidence_file(
        &strategy_input_path,
        strategy_input_hash,
    )
    .expect("matching strategy input evidence should parse");

    assert!(!audit.is_approved());
    assert!(
        audit
            .block_reasons()
            .contains(&Phase8CanaryBlockReason::NonPositiveSpotPrice)
    );

    std::fs::write(
        &strategy_input_path,
        r#"{"realized_volatility":"2.5","seconds_to_expiry":300,"spot_price":"100000.0","price_to_beat_value":"0","expected_edge_basis_points":"12.5","worst_case_edge_basis_points":"12.5","fee_rate_basis_points":"0","price_to_beat_source":"chainlink_data_streams","reference_quote_ts_event":1234567890}"#,
    )
    .expect("strategy input evidence should write");
    let strategy_input_hash = Phase8OperatorApprovalEnvelope::sha256_file(&strategy_input_path)
        .expect("strategy input evidence hash should compute");

    let audit = Phase8StrategyInputSafetyAudit::from_evidence_file(
        &strategy_input_path,
        strategy_input_hash,
    )
    .expect("matching strategy input evidence should parse");

    assert!(!audit.is_approved());
    assert!(
        audit
            .block_reasons()
            .contains(&Phase8CanaryBlockReason::NonPositivePriceToBeatValue)
    );
}

#[test]
fn strategy_audit_blocks_invalid_edge_or_fee_metrics() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let strategy_input_path = temp.path().join("phase8-strategy-input-evidence.json");
    std::fs::write(
        &strategy_input_path,
        r#"{"realized_volatility":"2.5","seconds_to_expiry":300,"spot_price":"100000.0","price_to_beat_value":"100000.0","expected_edge_basis_points":"12.5","worst_case_edge_basis_points":"0","fee_rate_basis_points":"0","price_to_beat_source":"chainlink_data_streams","reference_quote_ts_event":1234567890}"#,
    )
    .expect("strategy input evidence should write");
    let strategy_input_hash = Phase8OperatorApprovalEnvelope::sha256_file(&strategy_input_path)
        .expect("strategy input evidence hash should compute");

    let audit = Phase8StrategyInputSafetyAudit::from_evidence_file(
        &strategy_input_path,
        strategy_input_hash,
    )
    .expect("matching strategy input evidence should parse");

    assert!(!audit.is_approved());
    assert!(
        audit
            .block_reasons()
            .contains(&Phase8CanaryBlockReason::NonPositiveWorstCaseEdgeBasisPoints)
    );

    std::fs::write(
        &strategy_input_path,
        r#"{"realized_volatility":"2.5","seconds_to_expiry":300,"spot_price":"100000.0","price_to_beat_value":"100000.0","expected_edge_basis_points":"0","worst_case_edge_basis_points":"12.5","fee_rate_basis_points":"0","price_to_beat_source":"chainlink_data_streams","reference_quote_ts_event":1234567890}"#,
    )
    .expect("strategy input evidence should write");
    let strategy_input_hash = Phase8OperatorApprovalEnvelope::sha256_file(&strategy_input_path)
        .expect("strategy input evidence hash should compute");

    let audit = Phase8StrategyInputSafetyAudit::from_evidence_file(
        &strategy_input_path,
        strategy_input_hash,
    )
    .expect("matching strategy input evidence should parse");

    assert!(!audit.is_approved());
    assert!(
        audit
            .block_reasons()
            .contains(&Phase8CanaryBlockReason::NonPositiveExpectedEdgeBasisPoints)
    );

    std::fs::write(
        &strategy_input_path,
        r#"{"realized_volatility":"2.5","seconds_to_expiry":300,"spot_price":"100000.0","price_to_beat_value":"100000.0","expected_edge_basis_points":"12.5","worst_case_edge_basis_points":"12.5","fee_rate_basis_points":"-0.1","price_to_beat_source":"chainlink_data_streams","reference_quote_ts_event":1234567890}"#,
    )
    .expect("strategy input evidence should write");
    let strategy_input_hash = Phase8OperatorApprovalEnvelope::sha256_file(&strategy_input_path)
        .expect("strategy input evidence hash should compute");

    let audit = Phase8StrategyInputSafetyAudit::from_evidence_file(
        &strategy_input_path,
        strategy_input_hash,
    )
    .expect("matching strategy input evidence should parse");

    assert!(!audit.is_approved());
    assert!(
        audit
            .block_reasons()
            .contains(&Phase8CanaryBlockReason::NegativeFeeRateBasisPoints)
    );
}

#[test]
fn strategy_audit_blocks_missing_source_or_reference_timestamp() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let strategy_input_path = temp.path().join("phase8-strategy-input-evidence.json");
    std::fs::write(
        &strategy_input_path,
        r#"{"realized_volatility":"2.5","seconds_to_expiry":300,"spot_price":"100000.0","price_to_beat_value":"100000.0","expected_edge_basis_points":"12.5","worst_case_edge_basis_points":"12.5","fee_rate_basis_points":"0","price_to_beat_source":"","reference_quote_ts_event":1234567890}"#,
    )
    .expect("strategy input evidence should write");
    let strategy_input_hash = Phase8OperatorApprovalEnvelope::sha256_file(&strategy_input_path)
        .expect("strategy input evidence hash should compute");

    let audit = Phase8StrategyInputSafetyAudit::from_evidence_file(
        &strategy_input_path,
        strategy_input_hash,
    )
    .expect("matching strategy input evidence should parse");

    assert!(!audit.is_approved());
    assert!(
        audit
            .block_reasons()
            .contains(&Phase8CanaryBlockReason::MissingPriceToBeatSource)
    );

    std::fs::write(
        &strategy_input_path,
        r#"{"realized_volatility":"2.5","seconds_to_expiry":300,"spot_price":"100000.0","price_to_beat_value":"100000.0","expected_edge_basis_points":"12.5","worst_case_edge_basis_points":"12.5","fee_rate_basis_points":"0","price_to_beat_source":"chainlink_data_streams","reference_quote_ts_event":0}"#,
    )
    .expect("strategy input evidence should write");
    let strategy_input_hash = Phase8OperatorApprovalEnvelope::sha256_file(&strategy_input_path)
        .expect("strategy input evidence hash should compute");

    let audit = Phase8StrategyInputSafetyAudit::from_evidence_file(
        &strategy_input_path,
        strategy_input_hash,
    )
    .expect("matching strategy input evidence should parse");

    assert!(!audit.is_approved());
    assert!(
        audit
            .block_reasons()
            .contains(&Phase8CanaryBlockReason::MissingReferenceQuoteTsEvent)
    );
}

#[test]
fn strategy_audit_verifies_input_evidence_hash_before_approving() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let evidence_path = temp.path().join("strategy-input-evidence.json");
    std::fs::write(
        &evidence_path,
        r#"{"realized_volatility":"2.5","seconds_to_expiry":300,"spot_price":"100000.0","price_to_beat_value":"100000.0","expected_edge_basis_points":"12.5","worst_case_edge_basis_points":"12.5","fee_rate_basis_points":"0","price_to_beat_source":"chainlink_data_streams","reference_quote_ts_event":1234567890}"#,
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
        r#"{"realized_volatility":"2.5","seconds_to_expiry":300,"spot_price":"100000.0","price_to_beat_value":"100000.0","expected_edge_basis_points":"12.5","worst_case_edge_basis_points":"12.5","fee_rate_basis_points":"0","price_to_beat_source":"chainlink_data_streams","reference_quote_ts_event":1234567890}"#,
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
            post_run_hygiene_ref: Phase8EvidenceRef {
                path_hash: "9999999999999999999999999999999999999999999999999999999999999999"
                    .to_string(),
                record_hash: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                    .to_string(),
            },
        },
        1,
    )
    .expect("one admitted order should produce live canary proof");

    assert_eq!(evidence.outcome, Phase8CanaryOutcome::LiveCanaryProof);
    assert_eq!(evidence.submit_admission_ref.admitted_order_count, 1);
    assert!(evidence.block_reasons.is_empty());
    assert!(evidence.live_order_ref.is_some());
    assert!(evidence.nt_submit_event_ref.is_some());
    assert!(evidence.venue_order_state_ref.is_some());
    assert!(evidence.strategy_cancel_ref.is_some());
    assert!(evidence.restart_reconciliation_ref.is_some());
    assert!(evidence.post_run_hygiene_ref.is_some());

    let rendered = serde_json::to_string(&evidence).expect("evidence should render");
    assert!(!rendered.contains("operator-approved-canary-001"));
    assert!(!rendered.contains("client-order-001"));
    assert!(rendered.contains("restart_reconciliation_ref"));
    assert!(rendered.contains("post_run_hygiene_ref"));
}

#[test]
fn live_canary_evidence_rejects_unconsumed_submit_admission_count() {
    let error = Phase8CanaryEvidence::live_canary_proof(
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
            post_run_hygiene_ref: Phase8EvidenceRef {
                path_hash: "9999999999999999999999999999999999999999999999999999999999999999"
                    .to_string(),
                record_hash: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                    .to_string(),
            },
        },
        0,
    )
    .expect_err("zero admitted orders must not produce live canary proof");

    assert!(
        error.to_string().contains("admitted_order_count"),
        "error should mention admitted order count: {error}"
    );
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
        financial_envelope_path: "phase8-financial-envelope.json".to_string(),
        financial_envelope_sha256: "expected-financial-envelope-hash".to_string(),
        pre_run_state_path: "phase8-pre-run-state.json".to_string(),
        pre_run_state_sha256: "expected-pre-run-state-hash".to_string(),
        abort_plan_path: "phase8-abort-plan.json".to_string(),
        abort_plan_sha256: "expected-abort-plan-hash".to_string(),
        operator_approval_id: "operator-approved-canary-001".to_string(),
        approval_not_before_unix_seconds: 1_000,
        approval_not_after_unix_seconds: 2_000,
        approval_nonce_path: "phase8-approval-nonce.json".to_string(),
        approval_nonce_sha256: "expected-approval-nonce-hash".to_string(),
        approval_consumption_path: "phase8-approval-consumed.json".to_string(),
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
fn operator_approval_envelope_consumes_time_bound_nonce_once() {
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
        r#"{"realized_volatility":"2.5","seconds_to_expiry":300,"spot_price":"100000.0","price_to_beat_value":"100000.0","expected_edge_basis_points":"12.5","worst_case_edge_basis_points":"12.5","fee_rate_basis_points":"0","price_to_beat_source":"chainlink_data_streams","reference_quote_ts_event":1234567890}"#,
    )
    .expect("strategy input evidence should write");
    let strategy_input_hash = Phase8OperatorApprovalEnvelope::sha256_file(&strategy_input_path)
        .expect("strategy input evidence hash should compute");
    let approval_nonce_path = temp.path().join("phase8-approval-nonce.json");
    std::fs::write(
        &approval_nonce_path,
        r#"{"record_kind":"phase8_operator_approval_nonce","nonce_hash":"dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"}"#,
    )
    .expect("approval nonce should write");
    let approval_nonce_hash = Phase8OperatorApprovalEnvelope::sha256_file(&approval_nonce_path)
        .expect("approval nonce hash should compute");
    let financial_envelope_path = temp.path().join("phase8-financial-envelope.json");
    write_phase8_financial_envelope(&financial_envelope_path, "0.25");
    let financial_envelope_hash =
        Phase8OperatorApprovalEnvelope::sha256_file(&financial_envelope_path)
            .expect("financial envelope hash should compute");
    let pre_run_state_path = temp.path().join("phase8-pre-run-state.json");
    write_phase8_pre_run_state(&pre_run_state_path, false);
    let pre_run_state_hash = Phase8OperatorApprovalEnvelope::sha256_file(&pre_run_state_path)
        .expect("pre-run state hash should compute");
    let abort_plan_path = temp.path().join("phase8-abort-plan.json");
    write_phase8_abort_plan(&abort_plan_path, false);
    let abort_plan_hash = Phase8OperatorApprovalEnvelope::sha256_file(&abort_plan_path)
        .expect("abort plan hash should compute");
    let approval_consumption_path = temp.path().join("phase8-approval-consumed.json");
    let loaded = loaded_with_live_canary("reports/no-submit-readiness.json");
    let envelope = Phase8OperatorApprovalEnvelope {
        head_sha: "expected-head".to_string(),
        root_toml_path: "config/live.local.toml".to_string(),
        root_toml_sha256: "expected-config-hash".to_string(),
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
        operator_approval_id: "operator-approved-canary-001".to_string(),
        approval_not_before_unix_seconds: 1_000,
        approval_not_after_unix_seconds: 2_000,
        approval_nonce_path: approval_nonce_path.to_string_lossy().to_string(),
        approval_nonce_sha256: approval_nonce_hash,
        approval_consumption_path: approval_consumption_path.to_string_lossy().to_string(),
        canary_evidence_path: "phase8-canary-evidence.json".to_string(),
    };

    let too_early_error = envelope
        .validate_and_consume_against(
            "expected-head",
            "expected-config-hash",
            "operator-approved-canary-001",
            &loaded,
            999,
        )
        .expect_err("approval before not_before should fail closed");
    assert!(
        too_early_error.to_string().contains("not yet valid"),
        "error should mention not-before window: {too_early_error}"
    );
    assert!(
        !approval_consumption_path.exists(),
        "rejected approval must not create consumption evidence"
    );

    let mut wrong_nonce_envelope = envelope.clone();
    wrong_nonce_envelope.approval_nonce_sha256 =
        "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee".to_string();
    let wrong_nonce_error = wrong_nonce_envelope
        .validate_and_consume_against(
            "expected-head",
            "expected-config-hash",
            "operator-approved-canary-001",
            &loaded,
            1_500,
        )
        .expect_err("nonce hash mismatch should fail closed");
    assert!(
        wrong_nonce_error.to_string().contains("nonce sha256"),
        "error should mention nonce hash mismatch: {wrong_nonce_error}"
    );
    assert!(
        !approval_consumption_path.exists(),
        "nonce mismatch must not create consumption evidence"
    );

    envelope
        .validate_and_consume_against(
            "expected-head",
            "expected-config-hash",
            "operator-approved-canary-001",
            &loaded,
            1_500,
        )
        .expect("first approval consumption inside time window should pass");
    assert!(
        approval_consumption_path.exists(),
        "approval consumption evidence should be created"
    );
    let consumption_json =
        std::fs::read_to_string(&approval_consumption_path).expect("consumption should read");
    assert!(
        !consumption_json.contains("operator-approved-canary-001"),
        "consumption evidence must not serialize raw approval id"
    );
    let consumption: Value =
        serde_json::from_str(&consumption_json).expect("consumption should parse as json");
    assert_eq!(
        consumption["record_kind"],
        "phase8_operator_approval_consumption"
    );
    assert_eq!(consumption["approval_not_before_unix_seconds"], 1_000);
    assert_eq!(consumption["approval_not_after_unix_seconds"], 2_000);
    assert_eq!(consumption["consumed_unix_seconds"], 1_500);

    let expired_after_consumption_error = envelope
        .validate_and_consume_against(
            "expected-head",
            "expected-config-hash",
            "operator-approved-canary-001",
            &loaded,
            2_001,
        )
        .expect_err("expired replay after consumption should fail closed as consumed");
    assert!(
        expired_after_consumption_error
            .to_string()
            .contains("already consumed"),
        "error should mention consumed approval replay: {expired_after_consumption_error}"
    );

    let error = envelope
        .validate_and_consume_against(
            "expected-head",
            "expected-config-hash",
            "operator-approved-canary-001",
            &loaded,
            1_500,
        )
        .expect_err("second approval consumption should fail closed");

    assert!(
        error.to_string().contains("already consumed"),
        "error should mention consumed approval replay: {error}"
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
        r#"{"realized_volatility":"2.5","seconds_to_expiry":300,"spot_price":"100000.0","price_to_beat_value":"100000.0","expected_edge_basis_points":"12.5","worst_case_edge_basis_points":"12.5","fee_rate_basis_points":"0","price_to_beat_source":"chainlink_data_streams","reference_quote_ts_event":1234567890}"#,
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
        financial_envelope_path: "phase8-financial-envelope.json".to_string(),
        financial_envelope_sha256: "expected-financial-envelope-hash".to_string(),
        pre_run_state_path: "phase8-pre-run-state.json".to_string(),
        pre_run_state_sha256: "expected-pre-run-state-hash".to_string(),
        abort_plan_path: "phase8-abort-plan.json".to_string(),
        abort_plan_sha256: "expected-abort-plan-hash".to_string(),
        operator_approval_id: "operator-approved-canary-001".to_string(),
        approval_not_before_unix_seconds: 1_000,
        approval_not_after_unix_seconds: 2_000,
        approval_nonce_path: "phase8-approval-nonce.json".to_string(),
        approval_nonce_sha256: "expected-approval-nonce-hash".to_string(),
        approval_consumption_path: "phase8-approval-consumed.json".to_string(),
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
        r#"{"realized_volatility":"2.5","seconds_to_expiry":300,"spot_price":"100000.0","price_to_beat_value":"100000.0","expected_edge_basis_points":"12.5","worst_case_edge_basis_points":"12.5","fee_rate_basis_points":"0","price_to_beat_source":"chainlink_data_streams","reference_quote_ts_event":1234567890}"#,
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
        financial_envelope_path: "phase8-financial-envelope.json".to_string(),
        financial_envelope_sha256: "expected-financial-envelope-hash".to_string(),
        pre_run_state_path: "phase8-pre-run-state.json".to_string(),
        pre_run_state_sha256: "expected-pre-run-state-hash".to_string(),
        abort_plan_path: "phase8-abort-plan.json".to_string(),
        abort_plan_sha256: "expected-abort-plan-hash".to_string(),
        operator_approval_id: "operator-approved-canary-001".to_string(),
        approval_not_before_unix_seconds: 1_000,
        approval_not_after_unix_seconds: 2_000,
        approval_nonce_path: "phase8-approval-nonce.json".to_string(),
        approval_nonce_sha256: "expected-approval-nonce-hash".to_string(),
        approval_consumption_path: "phase8-approval-consumed.json".to_string(),
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

#[test]
fn operator_approval_envelope_verifies_financial_envelope_hash_and_loaded_config() {
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
        r#"{"realized_volatility":"2.5","seconds_to_expiry":300,"spot_price":"100000.0","price_to_beat_value":"100000.0","expected_edge_basis_points":"12.5","worst_case_edge_basis_points":"12.5","fee_rate_basis_points":"0","price_to_beat_source":"chainlink_data_streams","reference_quote_ts_event":1234567890}"#,
    )
    .expect("strategy input evidence should write");
    let strategy_input_hash = Phase8OperatorApprovalEnvelope::sha256_file(&strategy_input_path)
        .expect("strategy input evidence hash should compute");
    let financial_envelope_path = temp.path().join("phase8-financial-envelope.json");
    std::fs::write(
        &financial_envelope_path,
        serde_json::to_vec(&serde_json::json!({
            "max_live_order_count": 1,
            "max_notional_per_order": "5.00",
            "strategy_instance_id": "bitcoin_updown_main",
            "strategy_venue": "polymarket_main",
            "configured_target_id": "btc_updown_5m",
            "target_kind": "rotating_market",
            "rotating_market_family": "updown",
            "underlying_asset": "BTC",
            "cadence_seconds": 300,
            "market_selection_rule": "active_or_next",
            "retry_interval_seconds": 5,
            "blocked_after_seconds": 60,
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
        }))
        .expect("financial envelope should serialize"),
    )
    .expect("financial envelope should write");
    let financial_envelope_hash =
        Phase8OperatorApprovalEnvelope::sha256_file(&financial_envelope_path)
            .expect("financial envelope hash should compute");
    let pre_run_state_path = temp.path().join("phase8-pre-run-state.json");
    write_phase8_pre_run_state(&pre_run_state_path, false);
    let pre_run_state_hash = Phase8OperatorApprovalEnvelope::sha256_file(&pre_run_state_path)
        .expect("pre-run state hash should compute");
    let abort_plan_path = temp.path().join("phase8-abort-plan.json");
    write_phase8_abort_plan(&abort_plan_path, false);
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
    let mut loaded = loaded_with_live_canary("reports/no-submit-readiness.json");
    loaded
        .root
        .live_canary
        .as_mut()
        .expect("live canary should exist")
        .max_notional_per_order = "5.00".to_string();
    let envelope = Phase8OperatorApprovalEnvelope {
        head_sha: "expected-head".to_string(),
        root_toml_path: "config/live.local.toml".to_string(),
        root_toml_sha256: "expected-config-hash".to_string(),
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
        operator_approval_id: "operator-approved-canary-001".to_string(),
        approval_not_before_unix_seconds: 1_000,
        approval_not_after_unix_seconds: 2_000,
        approval_nonce_path: approval_nonce_path.to_string_lossy().to_string(),
        approval_nonce_sha256: approval_nonce_hash,
        approval_consumption_path: approval_consumption_path.to_string_lossy().to_string(),
        canary_evidence_path: "phase8-canary-evidence.json".to_string(),
    };

    let mut wrong_hash_envelope = envelope.clone();
    wrong_hash_envelope.financial_envelope_sha256 =
        "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee".to_string();
    let wrong_hash_error = wrong_hash_envelope
        .validate_and_consume_against(
            "expected-head",
            "expected-config-hash",
            "operator-approved-canary-001",
            &loaded,
            1_500,
        )
        .expect_err("financial envelope hash mismatch should fail closed");
    assert!(
        wrong_hash_error
            .to_string()
            .contains("financial_envelope_sha256"),
        "error should mention financial envelope hash mismatch: {wrong_hash_error}"
    );
    assert!(
        !approval_consumption_path.exists(),
        "financial mismatch must not create consumption evidence"
    );

    let mut mismatched_loaded = loaded.clone();
    mismatched_loaded
        .root
        .live_canary
        .as_mut()
        .expect("live canary should exist")
        .max_notional_per_order = "4.00".to_string();
    let mismatched_config_error = envelope
        .validate_and_consume_against(
            "expected-head",
            "expected-config-hash",
            "operator-approved-canary-001",
            &mismatched_loaded,
            1_500,
        )
        .expect_err("financial envelope mismatch against loaded TOML should fail closed");
    assert!(
        mismatched_config_error
            .to_string()
            .contains("max_notional_per_order"),
        "error should mention mismatched financial field: {mismatched_config_error}"
    );
    assert!(
        !approval_consumption_path.exists(),
        "financial mismatch must not create consumption evidence"
    );

    let mut mismatched_impact_loaded = loaded.clone();
    let runtime_parameters = mismatched_impact_loaded.strategies[0]
        .config
        .parameters
        .as_table_mut()
        .and_then(|parameters| parameters.get_mut("runtime"))
        .and_then(toml::Value::as_table_mut)
        .expect("strategy runtime parameters should be a TOML table");
    runtime_parameters.insert("book_impact_cap_bps".to_string(), toml::Value::Integer(49));
    let mismatched_impact_error = envelope
        .validate_and_consume_against(
            "expected-head",
            "expected-config-hash",
            "operator-approved-canary-001",
            &mismatched_impact_loaded,
            1_500,
        )
        .expect_err("book impact cap mismatch against loaded TOML should fail closed");
    assert!(
        mismatched_impact_error
            .to_string()
            .contains("phase8 financial envelope `book_impact_cap_bps` does not match loaded TOML"),
        "error should mention mismatched book impact cap: {mismatched_impact_error}"
    );
    assert!(
        !approval_consumption_path.exists(),
        "book impact cap mismatch must not create consumption evidence"
    );

    let mut mismatched_retry_loaded = loaded.clone();
    let target = mismatched_retry_loaded.strategies[0]
        .config
        .target
        .as_table_mut()
        .expect("strategy target should be a TOML table");
    target.insert(
        "retry_interval_seconds".to_string(),
        toml::Value::Integer(6),
    );
    let mismatched_retry_error = envelope
        .validate_and_consume_against(
            "expected-head",
            "expected-config-hash",
            "operator-approved-canary-001",
            &mismatched_retry_loaded,
            1_500,
        )
        .expect_err("target retry window mismatch against loaded TOML should fail closed");
    assert!(
        mismatched_retry_error.to_string().contains(
            "phase8 financial envelope `retry_interval_seconds` does not match loaded TOML"
        ),
        "error should mention mismatched retry window: {mismatched_retry_error}"
    );
    assert!(
        !approval_consumption_path.exists(),
        "target retry window mismatch must not create consumption evidence"
    );

    let mut mismatched_block_loaded = loaded.clone();
    let target = mismatched_block_loaded.strategies[0]
        .config
        .target
        .as_table_mut()
        .expect("strategy target should be a TOML table");
    target.insert(
        "blocked_after_seconds".to_string(),
        toml::Value::Integer(61),
    );
    let mismatched_block_error = envelope
        .validate_and_consume_against(
            "expected-head",
            "expected-config-hash",
            "operator-approved-canary-001",
            &mismatched_block_loaded,
            1_500,
        )
        .expect_err("target blocked window mismatch against loaded TOML should fail closed");
    assert!(
        mismatched_block_error.to_string().contains(
            "phase8 financial envelope `blocked_after_seconds` does not match loaded TOML"
        ),
        "error should mention mismatched blocked window: {mismatched_block_error}"
    );
    assert!(
        !approval_consumption_path.exists(),
        "target blocked window mismatch must not create consumption evidence"
    );

    let mut mismatched_edge_loaded = loaded.clone();
    let parameters = mismatched_edge_loaded.strategies[0]
        .config
        .parameters
        .as_table_mut()
        .expect("strategy parameters should be a TOML table");
    parameters.insert(
        "edge_threshold_basis_points".to_string(),
        toml::Value::Integer(101),
    );
    let mismatched_edge_error = envelope
        .validate_and_consume_against(
            "expected-head",
            "expected-config-hash",
            "operator-approved-canary-001",
            &mismatched_edge_loaded,
            1_500,
        )
        .expect_err("edge threshold mismatch against loaded TOML should fail closed");
    assert!(
        mismatched_edge_error.to_string().contains(
            "phase8 financial envelope `edge_threshold_basis_points` does not match loaded TOML"
        ),
        "error should mention mismatched edge threshold: {mismatched_edge_error}"
    );
    assert!(
        !approval_consumption_path.exists(),
        "edge threshold mismatch must not create consumption evidence"
    );

    let mut mismatched_entry_order_loaded = loaded.clone();
    let entry_order = mismatched_entry_order_loaded.strategies[0]
        .config
        .parameters
        .as_table_mut()
        .and_then(|parameters| parameters.get_mut("entry_order"))
        .and_then(toml::Value::as_table_mut)
        .expect("strategy entry order parameters should be a TOML table");
    entry_order.insert(
        "time_in_force".to_string(),
        toml::Value::String("gtc".to_string()),
    );
    let mismatched_entry_order_error = envelope
        .validate_and_consume_against(
            "expected-head",
            "expected-config-hash",
            "operator-approved-canary-001",
            &mismatched_entry_order_loaded,
            1_500,
        )
        .expect_err("entry order mismatch against loaded TOML should fail closed");
    assert!(
        mismatched_entry_order_error
            .to_string()
            .contains("phase8 financial envelope `entry_time_in_force` does not match loaded TOML"),
        "error should mention mismatched entry order field: {mismatched_entry_order_error}"
    );
    assert!(
        !approval_consumption_path.exists(),
        "entry order mismatch must not create consumption evidence"
    );

    let mut mismatched_exit_order_loaded = loaded.clone();
    let exit_order = mismatched_exit_order_loaded.strategies[0]
        .config
        .parameters
        .as_table_mut()
        .and_then(|parameters| parameters.get_mut("exit_order"))
        .and_then(toml::Value::as_table_mut)
        .expect("strategy exit order parameters should be a TOML table");
    exit_order.insert("is_reduce_only".to_string(), toml::Value::Boolean(true));
    let mismatched_exit_order_error = envelope
        .validate_and_consume_against(
            "expected-head",
            "expected-config-hash",
            "operator-approved-canary-001",
            &mismatched_exit_order_loaded,
            1_500,
        )
        .expect_err("exit order mismatch against loaded TOML should fail closed");
    assert!(
        mismatched_exit_order_error
            .to_string()
            .contains("phase8 financial envelope `exit_is_reduce_only` does not match loaded TOML"),
        "error should mention mismatched exit order field: {mismatched_exit_order_error}"
    );
    assert!(
        !approval_consumption_path.exists(),
        "exit order mismatch must not create consumption evidence"
    );

    envelope
        .validate_and_consume_against(
            "expected-head",
            "expected-config-hash",
            "operator-approved-canary-001",
            &loaded,
            1_500,
        )
        .expect("matching financial envelope should pass and consume approval");
    assert!(
        approval_consumption_path.exists(),
        "matching financial envelope should create consumption evidence"
    );
}

#[test]
fn operator_approval_envelope_verifies_pre_run_state_hash_and_required_clearances() {
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
        r#"{"realized_volatility":"2.5","seconds_to_expiry":300,"spot_price":"100000.0","price_to_beat_value":"100000.0","expected_edge_basis_points":"12.5","worst_case_edge_basis_points":"12.5","fee_rate_basis_points":"0","price_to_beat_source":"chainlink_data_streams","reference_quote_ts_event":1234567890}"#,
    )
    .expect("strategy input evidence should write");
    let strategy_input_hash = Phase8OperatorApprovalEnvelope::sha256_file(&strategy_input_path)
        .expect("strategy input evidence hash should compute");
    let financial_envelope_path = temp.path().join("phase8-financial-envelope.json");
    write_phase8_financial_envelope(&financial_envelope_path, "0.25");
    let financial_envelope_hash =
        Phase8OperatorApprovalEnvelope::sha256_file(&financial_envelope_path)
            .expect("financial envelope hash should compute");
    let pre_run_state_path = temp.path().join("phase8-pre-run-state.json");
    write_phase8_pre_run_state(&pre_run_state_path, false);
    let pre_run_state_hash = Phase8OperatorApprovalEnvelope::sha256_file(&pre_run_state_path)
        .expect("pre-run state hash should compute");
    let abort_plan_path = temp.path().join("phase8-abort-plan.json");
    write_phase8_abort_plan(&abort_plan_path, false);
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
    let loaded = loaded_with_live_canary("reports/no-submit-readiness.json");
    let envelope = Phase8OperatorApprovalEnvelope {
        head_sha: "expected-head".to_string(),
        root_toml_path: "config/live.local.toml".to_string(),
        root_toml_sha256: "expected-config-hash".to_string(),
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
        operator_approval_id: "operator-approved-canary-001".to_string(),
        approval_not_before_unix_seconds: 1_000,
        approval_not_after_unix_seconds: 2_000,
        approval_nonce_path: approval_nonce_path.to_string_lossy().to_string(),
        approval_nonce_sha256: approval_nonce_hash,
        approval_consumption_path: approval_consumption_path.to_string_lossy().to_string(),
        canary_evidence_path: "phase8-canary-evidence.json".to_string(),
    };

    let mut wrong_hash_envelope = envelope.clone();
    wrong_hash_envelope.pre_run_state_sha256 =
        "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee".to_string();
    let wrong_hash_error = wrong_hash_envelope
        .validate_and_consume_against(
            "expected-head",
            "expected-config-hash",
            "operator-approved-canary-001",
            &loaded,
            1_500,
        )
        .expect_err("pre-run state hash mismatch should fail closed");
    assert!(
        wrong_hash_error
            .to_string()
            .contains("pre_run_state_sha256"),
        "error should mention pre-run state hash mismatch: {wrong_hash_error}"
    );
    assert!(
        !approval_consumption_path.exists(),
        "pre-run state mismatch must not create consumption evidence"
    );

    write_phase8_pre_run_state(&pre_run_state_path, true);
    let blocked_pre_run_state_hash =
        Phase8OperatorApprovalEnvelope::sha256_file(&pre_run_state_path)
            .expect("pre-run state hash should compute");
    let mut blocked_envelope = envelope.clone();
    blocked_envelope.pre_run_state_sha256 = blocked_pre_run_state_hash;
    let blocked_error = blocked_envelope
        .validate_and_consume_against(
            "expected-head",
            "expected-config-hash",
            "operator-approved-canary-001",
            &loaded,
            1_500,
        )
        .expect_err("unsafe pre-run state should fail closed");
    assert!(
        blocked_error
            .to_string()
            .contains("preexisting_position_absent"),
        "error should mention blocked pre-run clearance: {blocked_error}"
    );
    assert!(
        !approval_consumption_path.exists(),
        "unsafe pre-run state must not create consumption evidence"
    );

    write_phase8_pre_run_state(&pre_run_state_path, false);
    envelope
        .validate_and_consume_against(
            "expected-head",
            "expected-config-hash",
            "operator-approved-canary-001",
            &loaded,
            1_500,
        )
        .expect("matching pre-run state should pass and consume approval");
    assert!(
        approval_consumption_path.exists(),
        "matching pre-run state should create consumption evidence"
    );
}

#[test]
fn operator_approval_envelope_verifies_abort_plan_hash_and_required_paths() {
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
        r#"{"realized_volatility":"2.5","seconds_to_expiry":300,"spot_price":"100000.0","price_to_beat_value":"100000.0","expected_edge_basis_points":"12.5","worst_case_edge_basis_points":"12.5","fee_rate_basis_points":"0","price_to_beat_source":"chainlink_data_streams","reference_quote_ts_event":1234567890}"#,
    )
    .expect("strategy input evidence should write");
    let strategy_input_hash = Phase8OperatorApprovalEnvelope::sha256_file(&strategy_input_path)
        .expect("strategy input evidence hash should compute");
    let financial_envelope_path = temp.path().join("phase8-financial-envelope.json");
    write_phase8_financial_envelope(&financial_envelope_path, "0.25");
    let financial_envelope_hash =
        Phase8OperatorApprovalEnvelope::sha256_file(&financial_envelope_path)
            .expect("financial envelope hash should compute");
    let pre_run_state_path = temp.path().join("phase8-pre-run-state.json");
    write_phase8_pre_run_state(&pre_run_state_path, false);
    let pre_run_state_hash = Phase8OperatorApprovalEnvelope::sha256_file(&pre_run_state_path)
        .expect("pre-run state hash should compute");
    let abort_plan_path = temp.path().join("phase8-abort-plan.json");
    write_phase8_abort_plan(&abort_plan_path, false);
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
    let loaded = loaded_with_live_canary("reports/no-submit-readiness.json");
    let envelope = Phase8OperatorApprovalEnvelope {
        head_sha: "expected-head".to_string(),
        root_toml_path: "config/live.local.toml".to_string(),
        root_toml_sha256: "expected-config-hash".to_string(),
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
        operator_approval_id: "operator-approved-canary-001".to_string(),
        approval_not_before_unix_seconds: 1_000,
        approval_not_after_unix_seconds: 2_000,
        approval_nonce_path: approval_nonce_path.to_string_lossy().to_string(),
        approval_nonce_sha256: approval_nonce_hash,
        approval_consumption_path: approval_consumption_path.to_string_lossy().to_string(),
        canary_evidence_path: "phase8-canary-evidence.json".to_string(),
    };

    let mut wrong_hash_envelope = envelope.clone();
    wrong_hash_envelope.abort_plan_sha256 =
        "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee".to_string();
    let wrong_hash_error = wrong_hash_envelope
        .validate_and_consume_against(
            "expected-head",
            "expected-config-hash",
            "operator-approved-canary-001",
            &loaded,
            1_500,
        )
        .expect_err("abort plan hash mismatch should fail closed");
    assert!(
        wrong_hash_error.to_string().contains("abort_plan_sha256"),
        "error should mention abort plan hash mismatch: {wrong_hash_error}"
    );
    assert!(
        !approval_consumption_path.exists(),
        "abort plan mismatch must not create consumption evidence"
    );

    write_phase8_abort_plan(&abort_plan_path, true);
    let blocked_abort_plan_hash = Phase8OperatorApprovalEnvelope::sha256_file(&abort_plan_path)
        .expect("abort plan hash should compute");
    let mut blocked_envelope = envelope.clone();
    blocked_envelope.abort_plan_sha256 = blocked_abort_plan_hash;
    let blocked_error = blocked_envelope
        .validate_and_consume_against(
            "expected-head",
            "expected-config-hash",
            "operator-approved-canary-001",
            &loaded,
            1_500,
        )
        .expect_err("unsafe abort plan should fail closed");
    assert!(
        blocked_error
            .to_string()
            .contains("panic_gate_trip_abort_defined"),
        "error should mention blocked abort policy: {blocked_error}"
    );
    assert!(
        !approval_consumption_path.exists(),
        "unsafe abort plan must not create consumption evidence"
    );

    write_phase8_abort_plan(&abort_plan_path, false);
    envelope
        .validate_and_consume_against(
            "expected-head",
            "expected-config-hash",
            "operator-approved-canary-001",
            &loaded,
            1_500,
        )
        .expect("matching abort plan should pass and consume approval");
    assert!(
        approval_consumption_path.exists(),
        "matching abort plan should create consumption evidence"
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

fn write_phase8_financial_envelope(path: &std::path::Path, max_notional_per_order: &str) {
    let json = serde_json::json!({
        "max_live_order_count": 1,
        "max_notional_per_order": max_notional_per_order,
        "strategy_instance_id": "bitcoin_updown_main",
        "strategy_venue": "polymarket_main",
        "configured_target_id": "btc_updown_5m",
        "target_kind": "rotating_market",
        "rotating_market_family": "updown",
        "underlying_asset": "BTC",
        "cadence_seconds": 300,
        "market_selection_rule": "active_or_next",
        "retry_interval_seconds": 5,
        "blocked_after_seconds": 60,
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

fn write_phase8_pre_run_state(path: &std::path::Path, has_preexisting_position: bool) {
    let json = serde_json::json!({
        "strategy_venue": "polymarket_main",
        "configured_target_id": "btc_updown_5m",
        "host_clock_skew_within_bound": true,
        "conflicting_open_orders_absent": true,
        "preexisting_position_absent": !has_preexisting_position,
        "market_state_approved": true,
        "market_window_approved": true,
        "funding_margin_covers_max_notional_plus_fees": true,
        "single_runner_lock_acquired": true,
        "egress_identity_approved": true
    });
    std::fs::write(
        path,
        serde_json::to_vec(&json).expect("pre-run state should serialize"),
    )
    .expect("pre-run state should write");
}

fn write_phase8_abort_plan(path: &std::path::Path, panic_policy_missing: bool) {
    let json = serde_json::json!({
        "strategy_venue": "polymarket_main",
        "configured_target_id": "btc_updown_5m",
        "cancel_if_open_defined": true,
        "nt_accepted_venue_pending_abort_defined": true,
        "partial_fill_abort_defined": true,
        "network_partition_during_submit_abort_defined": true,
        "panic_gate_trip_abort_defined": !panic_policy_missing
    });
    std::fs::write(
        path,
        serde_json::to_vec(&json).expect("abort plan should serialize"),
    )
    .expect("abort plan should write");
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
