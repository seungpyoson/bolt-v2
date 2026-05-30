mod support;

use bolt_v2::{
    bolt_v3_config::{LiveCanaryBlock, LoadedBoltV3Config, load_bolt_v3_config},
    bolt_v3_live_canary_gate::check_bolt_v3_live_canary_gate,
    bolt_v3_no_submit_readiness_schema::{
        APPROVAL_CONSUMPTION_RECORD_KIND, APPROVAL_ID_HASH_KEY, CONFIG_BUNDLE_CHECKSUM_KEY,
        CONTROLLED_CONNECT_STAGE, CONTROLLED_DISCONNECT_STAGE, EXECUTABLE_IDENTITY_KEY,
        GENERATED_AT_UNIX_SECONDS_KEY, LIVE_NODE_BUILD_STAGE, NO_SUBMIT_READINESS_SCHEMA_VERSION,
        OPERATOR_APPROVAL_STAGE, REFERENCE_READINESS_STAGE, REPORT_WRITE_STAGE, SCHEMA_VERSION_KEY,
        SECRET_RESOLUTION_STAGE, STAGE_KEY, STAGES_KEY, STATUS_KEY, STATUS_SATISFIED,
    },
    bolt_v3_tiny_canary_evidence::{
        Phase8CanaryBlockReason, Phase8CanaryEvidence, Phase8CanaryOutcome,
        Phase8CanaryPreflightStatus, Phase8EvidenceRef, Phase8LiveCanaryResultRefs,
        Phase8LiveOrderRef, Phase8NtLifecycleRef, Phase8OperatorApprovalEnvelope,
        Phase8StrategyInputSafetyAudit, Phase8StrategyInputSafetyInputs,
        evaluate_phase8_canary_preflight,
    },
};
use nautilus_model::enums::OmsType;
use rust_decimal::Decimal;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::Path;

const PHASE8_TEST_PRICE_TO_BEAT_SOURCE: &str = "chainlink_data_streams.configured-reference-price";
const PHASE8_TEST_GATE_SESSION_HASH: &str =
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const PHASE8_TEST_SELECTED_MARKET_KEY: &str =
    "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const PHASE8_TEST_GATE_NORMALIZED_VALUE_HASH: &str =
    "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
const PHASE8_TEST_GATE_PROVIDER_PROVENANCE_HASH: &str =
    "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";
const PHASE8_TEST_GATE_ARTIFACT_HASH: &str =
    "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";
const PHASE8_TEST_APPROVAL_ENVELOPE_SHA256: &str =
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
fn phase8_required_operator_artifact_terms() -> [&'static str; 24] {
    [
        "BOLT_V3_PHASE8_HEAD_SHA",
        "BOLT_V3_PHASE8_ROOT_TOML_PATH",
        "BOLT_V3_PHASE8_SSM_MANIFEST_PATH",
        "BOLT_V3_PHASE8_SSM_MANIFEST_SHA256",
        "BOLT_V3_PHASE8_STRATEGY_INPUT_EVIDENCE_PATH",
        "BOLT_V3_PHASE8_STRATEGY_INPUT_EVIDENCE_SHA256",
        "BOLT_V3_PHASE8_FINANCIAL_ENVELOPE_PATH",
        "BOLT_V3_PHASE8_FINANCIAL_ENVELOPE_SHA256",
        "BOLT_V3_PHASE8_PRE_RUN_STATE_PATH",
        "BOLT_V3_PHASE8_PRE_RUN_STATE_SHA256",
        "BOLT_V3_PHASE8_ABORT_PLAN_PATH",
        "BOLT_V3_PHASE8_ABORT_PLAN_SHA256",
        "BOLT_V3_PHASE8_OPERATOR_APPROVAL_ID",
        "BOLT_V3_PHASE8_APPROVAL_NOT_BEFORE_UNIX_SECONDS",
        "BOLT_V3_PHASE8_APPROVAL_NOT_AFTER_UNIX_SECONDS",
        "BOLT_V3_PHASE8_APPROVAL_NONCE_PATH",
        "BOLT_V3_PHASE8_APPROVAL_NONCE_SHA256",
        "BOLT_V3_PHASE8_APPROVAL_CONSUMPTION_PATH",
        "BOLT_V3_PHASE8_EVIDENCE_PATH",
        "BOLT_V3_PHASE8_DECISION_EVIDENCE_PATH",
        "BOLT_V3_PHASE8_NT_SUBMIT_EVENT_PATH",
        "BOLT_V3_PHASE8_VENUE_ORDER_STATE_PATH",
        "BOLT_V3_PHASE8_RESTART_RECONCILIATION_PATH",
        "BOLT_V3_PHASE8_POST_RUN_HYGIENE_PATH",
    ]
}

fn strategy_audit_from_evidence_file(
    path: impl AsRef<std::path::Path>,
    expected_sha256: impl AsRef<str>,
) -> anyhow::Result<Phase8StrategyInputSafetyAudit> {
    Phase8StrategyInputSafetyAudit::from_evidence_file(
        path,
        expected_sha256,
        PHASE8_TEST_PRICE_TO_BEAT_SOURCE,
    )
}

fn write_current_market_selection_source(
    dir: &Path,
) -> anyhow::Result<(std::path::PathBuf, String)> {
    let source_path = dir.join("current-market-selection-source.json");
    std::fs::write(
        &source_path,
        serde_json::to_vec(&serde_json::json!({
            "record_kind": "market_selection_result",
            "source": "nt_runtime_selection_snapshot",
            "market_selection_timestamp_ms": 1234567890_u64,
            "candidate_market_start_timestamps_ms": [1234567000_u64],
            "market_selection_outcome": "current",
            "polymarket_condition_id": "configured-condition",
            "polymarket_market_slug": "configured-asset-updown-configuredwindow",
            "polymarket_question_id": "configured-question",
            "up_instrument_id": "configured-condition-UP.POLYMARKET",
            "down_instrument_id": "configured-condition-DOWN.POLYMARKET",
            "selected_market_observed_timestamp_ms": 1234567890_u64,
            "polymarket_market_start_timestamp_ms": 1234567000_u64,
            "polymarket_market_end_timestamp_ms": 1234867000_u64
        }))?,
    )?;
    let source_hash = Phase8OperatorApprovalEnvelope::sha256_file(&source_path)?;
    Ok((source_path, source_hash))
}

fn current_strategy_input_evidence_json(
    price_to_beat_source: &str,
    market_selection_source_path: &Path,
    market_selection_source_sha256: &str,
) -> Value {
    serde_json::json!({
        "realized_volatility": "2.5",
        "seconds_to_market_end": 300_u64,
        "spot_price": "100000.0",
        "price_to_beat_value": "100000.0",
        "expected_edge_basis_points": "12.5",
        "worst_case_edge_basis_points": "12.5",
        "fee_rate_basis_points": "0",
        "price_to_beat_source": price_to_beat_source,
        "gate_session_hash": PHASE8_TEST_GATE_SESSION_HASH,
        "selected_market_key": PHASE8_TEST_SELECTED_MARKET_KEY,
        "gate_evidence": {
            "resolution": {
                "satisfaction_kind": "evidence",
                "selected_market_key": PHASE8_TEST_SELECTED_MARKET_KEY,
                "provider_id": "chainlink_main",
                "provider_kind": "chainlink_data_streams",
                "value_kind": "scalar_price",
                "normalized_value_sha256": PHASE8_TEST_GATE_NORMALIZED_VALUE_HASH,
                "provider_provenance_sha256": PHASE8_TEST_GATE_PROVIDER_PROVENANCE_HASH,
                "artifact_sha256s": [PHASE8_TEST_GATE_ARTIFACT_HASH]
            }
        },
        "reference_quote_ts_event": 1234567890_u64,
        "pricing_kurtosis": "0",
        "theta_decay_factor": "0",
        "theta_scaled_min_edge_bps": "12.5",
        "market_selection_timestamp_ms": 1234567890_u64,
        "market_selection_source_path": market_selection_source_path.to_string_lossy(),
        "market_selection_source_sha256": market_selection_source_sha256,
        "market_selection_outcome": "current",
        "polymarket_condition_id": "configured-condition",
        "polymarket_market_slug": "configured-asset-updown-configuredwindow",
        "polymarket_question_id": "configured-question",
        "up_instrument_id": "configured-condition-UP.POLYMARKET",
        "down_instrument_id": "configured-condition-DOWN.POLYMARKET",
        "selected_market_observed_timestamp_ms": 1234567890_u64,
        "polymarket_market_start_timestamp_ms": 1234567000_u64,
        "polymarket_market_end_timestamp_ms": 1234867000_u64
    })
}

fn remove_strategy_input_readiness_identity(value: &mut Value) {
    let object = value
        .as_object_mut()
        .expect("strategy input evidence should be a JSON object");
    object.remove("gate_session_hash");
    object.remove("selected_market_key");
    object.remove("gate_evidence");
}

fn write_current_strategy_input_evidence(
    path: &Path,
    price_to_beat_source: &str,
    market_selection_source_path: &Path,
    market_selection_source_sha256: &str,
) -> anyhow::Result<()> {
    std::fs::write(
        path,
        serde_json::to_vec(&current_strategy_input_evidence_json(
            price_to_beat_source,
            market_selection_source_path,
            market_selection_source_sha256,
        ))?,
    )?;
    Ok(())
}

#[test]
fn tiny_canary_quickstart_names_required_operator_artifacts() {
    let quickstart = include_str!("../specs/001-thin-live-canary-path/quickstart.md");

    for term in phase8_required_operator_artifact_terms() {
        assert!(
            quickstart.contains(term),
            "phase8 quickstart must name required operator artifact `{term}`"
        );
    }
    assert!(!quickstart.contains("BOLT_V3_PHASE8_CLIENT_ORDER_ID_HASH"));
    assert!(!quickstart.contains("BOLT_V3_PHASE8_VENUE_ORDER_ID_HASH"));
    assert!(!quickstart.contains("BOLT_V3_PHASE8_ROOT_TOML_SHA256"));
    assert!(!quickstart.contains("BOLT_V3_PHASE8_APPROVAL_ENVELOPE_SHA256"));
}

#[test]
fn tiny_canary_schema_doc_names_required_operator_artifacts() {
    let schema_doc = include_str!("../docs/bolt-v3/2026-04-25-bolt-v3-schema.md");

    for term in phase8_required_operator_artifact_terms() {
        assert!(
            schema_doc.contains(term),
            "phase8 schema doc must name required operator artifact `{term}`"
        );
    }
    assert!(!schema_doc.contains("BOLT_V3_PHASE8_CLIENT_ORDER_ID_HASH"));
    assert!(!schema_doc.contains("BOLT_V3_PHASE8_VENUE_ORDER_ID_HASH"));
    assert!(!schema_doc.contains("BOLT_V3_PHASE8_ROOT_TOML_SHA256"));
    assert!(!schema_doc.contains("BOLT_V3_PHASE8_APPROVAL_ENVELOPE_SHA256"));
}

#[test]
fn operator_approval_env_does_not_require_circular_hash_env_vars() {
    let source = support::repo_text("src/bolt_v3_tiny_canary_evidence.rs");
    let from_env = source
        .split("pub fn from_env() -> Result<Self>")
        .nth(1)
        .and_then(|tail| tail.split("pub fn validate_against").next())
        .expect("Phase8OperatorApprovalEnvelope::from_env source should be present");

    assert!(
        !from_env.contains("BOLT_V3_PHASE8_ROOT_TOML_SHA256"),
        "from_env must compute root_toml_sha256 from BOLT_V3_PHASE8_ROOT_TOML_PATH"
    );
    assert!(
        !from_env.contains("BOLT_V3_PHASE8_APPROVAL_ENVELOPE_SHA256"),
        "from_env must read approval_envelope_sha256 from loaded TOML"
    );
    assert!(
        from_env.contains("root_toml_sha256: Self::sha256_file(&root_toml_path)?"),
        "from_env must compute the root TOML hash internally"
    );
    assert!(
        from_env.contains(
            "approval_envelope_sha256: operator_evidence.approval_envelope_sha256.clone()"
        ),
        "from_env must source approval_envelope_sha256 from `[live_canary].operator_evidence`"
    );
}

#[test]
fn tiny_canary_runtime_contract_does_not_prebind_live_order_ids() {
    let runtime_contract = include_str!("../docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md");
    let gate_section = runtime_contract
        .split("### 11.8 Live canary gate boundary")
        .nth(1)
        .and_then(|section| section.split("## 12. Panic Gate").next())
        .expect("runtime contract must contain live canary gate boundary section");

    assert!(
        !gate_section.contains("client_order_id_hash"),
        "live canary gate boundary must not require pre-run client order id hash"
    );
    assert!(
        !gate_section.contains("venue_order_id_hash"),
        "live canary gate boundary must not require pre-run venue order id hash"
    );
}

#[test]
fn tiny_canary_quickstart_names_conditional_strategy_cancel_artifact() {
    let quickstart = include_str!("../specs/001-thin-live-canary-path/quickstart.md");

    assert!(
        quickstart.contains("BOLT_V3_PHASE8_STRATEGY_CANCEL_PATH"),
        "phase8 quickstart must name conditional strategy cancel artifact"
    );
}

#[tokio::test]
async fn preflight_blocks_missing_phase7_report_before_build() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let approval_consumption_path = temp.path().join("phase8-approval-consumed.json");
    let mut loaded = loaded_with_live_canary("reports/missing-no-submit-readiness.json");
    bind_loaded_approval_consumption_path(&mut loaded, &approval_consumption_path);
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
    let approval_consumption_path = temp.path().join("phase8-approval-consumed.json");
    write_satisfied_no_submit_readiness_report(&report_path);
    let mut loaded = loaded_with_live_canary(report_path.to_str().expect("utf8 report path"));
    bind_loaded_approval_consumption_path(&mut loaded, &approval_consumption_path);

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
    let approval_consumption_path = temp.path().join("phase8-approval-consumed.json");
    write_satisfied_no_submit_readiness_report(&report_path);
    let mut loaded = loaded_with_live_canary(report_path.to_str().expect("utf8 report path"));
    bind_loaded_approval_consumption_path(&mut loaded, &approval_consumption_path);
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

#[tokio::test]
async fn preflight_blocks_missing_live_canary_with_explicit_block_reason() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let report_path = temp.path().join("no-submit-readiness.json");
    write_satisfied_no_submit_readiness_report(&report_path);
    let mut loaded = loaded_with_live_canary(report_path.to_str().expect("utf8 report path"));
    loaded.root.live_canary = None;

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
    assert!(!report.block_reasons.is_empty());
    assert_eq!(report.max_live_order_count, None);
    assert!(!report.can_enter_live_runner());
}

#[test]
fn strategy_audit_blocks_non_positive_realized_volatility() {
    let audit =
        Phase8StrategyInputSafetyAudit::from_strategy_inputs(Phase8StrategyInputSafetyInputs {
            realized_volatility: Decimal::ZERO,
            seconds_to_market_end: 300,
            spot_price: Decimal::new(100_000, 0),
            price_to_beat_value: Decimal::new(100_000, 0),
            expected_edge_basis_points: Decimal::new(125, 1),
            worst_case_edge_basis_points: Decimal::new(125, 1),
            theta_scaled_min_edge_bps: Decimal::new(125, 1),
            fee_rate_basis_points: Decimal::ZERO,
            price_to_beat_source: PHASE8_TEST_PRICE_TO_BEAT_SOURCE,
            expected_price_to_beat_source: PHASE8_TEST_PRICE_TO_BEAT_SOURCE,
            reference_quote_ts_event: 1_234_567_890,
            pricing_kurtosis: Decimal::ZERO,
            theta_decay_factor: Decimal::ZERO,
        });

    assert!(
        audit
            .block_reasons()
            .contains(&Phase8CanaryBlockReason::NonPositiveRealizedVolatility)
    );
    assert!(!audit.is_approved());
}

#[test]
fn strategy_audit_blocks_zero_time_to_market_end() {
    let audit =
        Phase8StrategyInputSafetyAudit::from_strategy_inputs(Phase8StrategyInputSafetyInputs {
            realized_volatility: Decimal::new(25, 1),
            seconds_to_market_end: 0,
            spot_price: Decimal::new(100_000, 0),
            price_to_beat_value: Decimal::new(100_000, 0),
            expected_edge_basis_points: Decimal::new(125, 1),
            worst_case_edge_basis_points: Decimal::new(125, 1),
            theta_scaled_min_edge_bps: Decimal::new(125, 1),
            fee_rate_basis_points: Decimal::ZERO,
            price_to_beat_source: PHASE8_TEST_PRICE_TO_BEAT_SOURCE,
            expected_price_to_beat_source: PHASE8_TEST_PRICE_TO_BEAT_SOURCE,
            reference_quote_ts_event: 1_234_567_890,
            pricing_kurtosis: Decimal::ZERO,
            theta_decay_factor: Decimal::ZERO,
        });

    assert!(
        audit
            .block_reasons()
            .contains(&Phase8CanaryBlockReason::NonPositiveTimeToMarketEnd)
    );
    assert!(!audit.is_approved());
}

#[test]
fn strategy_audit_uses_normalized_readiness_identity_not_price_source_string() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let evidence_path = temp.path().join("strategy-input.json");
    let (source_path, source_hash) =
        write_current_market_selection_source(temp.path()).expect("source should write");
    write_current_strategy_input_evidence(
        &evidence_path,
        "legacy_provider_specific_source",
        &source_path,
        &source_hash,
    )
    .expect("strategy input evidence should write");
    let evidence_hash = Phase8OperatorApprovalEnvelope::sha256_file(&evidence_path)
        .expect("strategy evidence should hash");

    let approved = Phase8StrategyInputSafetyAudit::from_evidence_file(
        &evidence_path,
        &evidence_hash,
        "operator_configured_source",
    )
    .expect("readiness identity should audit without legacy price-source equality");
    assert!(approved.is_approved());

    let mut source_string_only = current_strategy_input_evidence_json(
        PHASE8_TEST_PRICE_TO_BEAT_SOURCE,
        &source_path,
        &source_hash,
    );
    remove_strategy_input_readiness_identity(&mut source_string_only);
    std::fs::write(
        &evidence_path,
        serde_json::to_vec(&source_string_only)
            .expect("source-string-only evidence should serialize"),
    )
    .expect("source-string-only evidence should write");
    let evidence_hash = Phase8OperatorApprovalEnvelope::sha256_file(&evidence_path)
        .expect("strategy evidence should hash");

    let blocked = strategy_audit_from_evidence_file(&evidence_path, &evidence_hash)
        .expect("source-string-only evidence should still parse into a blocked audit");
    assert!(
        blocked
            .block_reasons()
            .contains(&Phase8CanaryBlockReason::DecisionEvidenceUnavailable)
    );
}

#[test]
fn strategy_audit_uses_market_end_field_name_for_time_remaining() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let evidence_path = temp.path().join("strategy-input.json");
    let (source_path, source_hash) =
        write_current_market_selection_source(temp.path()).expect("source should write");
    let current_evidence = serde_json::to_string(&current_strategy_input_evidence_json(
        PHASE8_TEST_PRICE_TO_BEAT_SOURCE,
        &source_path,
        &source_hash,
    ))
    .expect("strategy input evidence should serialize");
    std::fs::write(&evidence_path, &current_evidence)
        .expect("strategy input evidence should write");
    let evidence_hash = Phase8OperatorApprovalEnvelope::sha256_file(&evidence_path)
        .expect("strategy evidence should hash");

    let approved = strategy_audit_from_evidence_file(&evidence_path, &evidence_hash)
        .expect("seconds_to_market_end evidence should parse");
    assert!(approved.is_approved());

    let legacy_time_field = ["seconds_to", "_expiry"].concat();
    let legacy_evidence = current_evidence.replace("seconds_to_market_end", &legacy_time_field);
    std::fs::write(&evidence_path, legacy_evidence)
        .expect("legacy strategy input evidence should write");
    let legacy_hash = Phase8OperatorApprovalEnvelope::sha256_file(&evidence_path)
        .expect("legacy strategy evidence should hash");
    let error = strategy_audit_from_evidence_file(&evidence_path, &legacy_hash)
        .expect_err("legacy time-remaining field should be rejected as stale evidence vocabulary");
    assert!(
        error.to_string().contains("unknown field") || error.to_string().contains("missing field"),
        "stale field rejection should mention serde field mismatch: {error}"
    );
}

#[test]
fn strategy_audit_uses_unit_suffixed_selected_market_observation_timestamp() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let evidence_path = temp.path().join("strategy-input.json");
    let (source_path, source_hash) =
        write_current_market_selection_source(temp.path()).expect("source should write");
    let current_evidence = serde_json::to_string(&current_strategy_input_evidence_json(
        PHASE8_TEST_PRICE_TO_BEAT_SOURCE,
        &source_path,
        &source_hash,
    ))
    .expect("strategy input evidence should serialize");
    std::fs::write(&evidence_path, &current_evidence)
        .expect("strategy input evidence should write");
    let evidence_hash = Phase8OperatorApprovalEnvelope::sha256_file(&evidence_path)
        .expect("strategy evidence should hash");

    let approved = strategy_audit_from_evidence_file(&evidence_path, &evidence_hash)
        .expect("unit-suffixed selected market timestamp evidence should parse");
    assert!(approved.is_approved());

    let legacy_timestamp_field = ["selected_market", "_observed", "_timestamp\":"].concat();
    let legacy_evidence = current_evidence.replace(
        "selected_market_observed_timestamp_ms\":",
        &legacy_timestamp_field,
    );
    std::fs::write(&evidence_path, legacy_evidence)
        .expect("legacy strategy input evidence should write");
    let legacy_hash = Phase8OperatorApprovalEnvelope::sha256_file(&evidence_path)
        .expect("legacy strategy evidence should hash");
    let error = strategy_audit_from_evidence_file(&evidence_path, &legacy_hash)
        .expect_err("legacy selected market timestamp field should be rejected");
    assert!(
        error.to_string().contains("unknown field") || error.to_string().contains("missing field"),
        "stale field rejection should mention serde field mismatch: {error}"
    );
}

#[test]
fn phase8_operator_artifacts_use_execution_client_id_field() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let financial_envelope_path = temp.path().join("phase8-financial-envelope.json");
    write_phase8_financial_envelope(&financial_envelope_path, "0.25");
    let envelope: serde_json::Value = serde_json::from_slice(
        &std::fs::read(&financial_envelope_path).expect("financial envelope should read"),
    )
    .expect("financial envelope should parse");
    assert_eq!(envelope["execution_client_id"], "polymarket_main");
    let legacy_execution_field = ["strategy", "_venue"].concat();
    assert!(envelope.get(&legacy_execution_field).is_none());
}

#[test]
fn strategy_audit_blocks_non_positive_spot_or_price_to_beat_evidence() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let strategy_input_path = temp.path().join("phase8-strategy-input-evidence.json");
    std::fs::write(
        &strategy_input_path,
        r#"{"realized_volatility":"2.5","seconds_to_market_end":300,"spot_price":"0","price_to_beat_value":"100000.0","expected_edge_basis_points":"12.5","worst_case_edge_basis_points":"12.5","fee_rate_basis_points":"0","price_to_beat_source":"chainlink_data_streams.configured-reference-price","reference_quote_ts_event":1234567890,"pricing_kurtosis":"0","theta_decay_factor":"0","theta_scaled_min_edge_bps":"12.5","market_selection_timestamp_ms":1234567890,"market_selection_outcome":"current","polymarket_condition_id":"configured-condition","polymarket_market_slug":"configured-asset-updown-configuredwindow","polymarket_question_id":"configured-question","up_instrument_id":"configured-condition-UP.POLYMARKET","down_instrument_id":"configured-condition-DOWN.POLYMARKET","selected_market_observed_timestamp_ms":1234567890,"polymarket_market_start_timestamp_ms":1234567000,"polymarket_market_end_timestamp_ms":1234867000}"#,
    )
    .expect("strategy input evidence should write");
    let strategy_input_hash = Phase8OperatorApprovalEnvelope::sha256_file(&strategy_input_path)
        .expect("strategy input evidence hash should compute");

    let audit = strategy_audit_from_evidence_file(&strategy_input_path, strategy_input_hash)
        .expect("matching strategy input evidence should parse");

    assert!(!audit.is_approved());
    assert!(
        audit
            .block_reasons()
            .contains(&Phase8CanaryBlockReason::NonPositiveSpotPrice)
    );

    std::fs::write(
        &strategy_input_path,
        r#"{"realized_volatility":"2.5","seconds_to_market_end":300,"spot_price":"100000.0","price_to_beat_value":"0","expected_edge_basis_points":"12.5","worst_case_edge_basis_points":"12.5","fee_rate_basis_points":"0","price_to_beat_source":"chainlink_data_streams.configured-reference-price","reference_quote_ts_event":1234567890,"pricing_kurtosis":"0","theta_decay_factor":"0","theta_scaled_min_edge_bps":"12.5","market_selection_timestamp_ms":1234567890,"market_selection_outcome":"current","polymarket_condition_id":"configured-condition","polymarket_market_slug":"configured-asset-updown-configuredwindow","polymarket_question_id":"configured-question","up_instrument_id":"configured-condition-UP.POLYMARKET","down_instrument_id":"configured-condition-DOWN.POLYMARKET","selected_market_observed_timestamp_ms":1234567890,"polymarket_market_start_timestamp_ms":1234567000,"polymarket_market_end_timestamp_ms":1234867000}"#,
    )
    .expect("strategy input evidence should write");
    let strategy_input_hash = Phase8OperatorApprovalEnvelope::sha256_file(&strategy_input_path)
        .expect("strategy input evidence hash should compute");

    let audit = strategy_audit_from_evidence_file(&strategy_input_path, strategy_input_hash)
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
        r#"{"realized_volatility":"2.5","seconds_to_market_end":300,"spot_price":"100000.0","price_to_beat_value":"100000.0","expected_edge_basis_points":"12.5","worst_case_edge_basis_points":"0","fee_rate_basis_points":"0","price_to_beat_source":"chainlink_data_streams.configured-reference-price","reference_quote_ts_event":1234567890,"pricing_kurtosis":"0","theta_decay_factor":"0","theta_scaled_min_edge_bps":"12.5","market_selection_timestamp_ms":1234567890,"market_selection_outcome":"current","polymarket_condition_id":"configured-condition","polymarket_market_slug":"configured-asset-updown-configuredwindow","polymarket_question_id":"configured-question","up_instrument_id":"configured-condition-UP.POLYMARKET","down_instrument_id":"configured-condition-DOWN.POLYMARKET","selected_market_observed_timestamp_ms":1234567890,"polymarket_market_start_timestamp_ms":1234567000,"polymarket_market_end_timestamp_ms":1234867000}"#,
    )
    .expect("strategy input evidence should write");
    let strategy_input_hash = Phase8OperatorApprovalEnvelope::sha256_file(&strategy_input_path)
        .expect("strategy input evidence hash should compute");

    let audit = strategy_audit_from_evidence_file(&strategy_input_path, strategy_input_hash)
        .expect("matching strategy input evidence should parse");

    assert!(!audit.is_approved());
    assert!(
        audit
            .block_reasons()
            .contains(&Phase8CanaryBlockReason::NonPositiveWorstCaseEdgeBasisPoints)
    );

    std::fs::write(
        &strategy_input_path,
        r#"{"realized_volatility":"2.5","seconds_to_market_end":300,"spot_price":"100000.0","price_to_beat_value":"100000.0","expected_edge_basis_points":"0","worst_case_edge_basis_points":"12.5","fee_rate_basis_points":"0","price_to_beat_source":"chainlink_data_streams.configured-reference-price","reference_quote_ts_event":1234567890,"pricing_kurtosis":"0","theta_decay_factor":"0","theta_scaled_min_edge_bps":"12.5","market_selection_timestamp_ms":1234567890,"market_selection_outcome":"current","polymarket_condition_id":"configured-condition","polymarket_market_slug":"configured-asset-updown-configuredwindow","polymarket_question_id":"configured-question","up_instrument_id":"configured-condition-UP.POLYMARKET","down_instrument_id":"configured-condition-DOWN.POLYMARKET","selected_market_observed_timestamp_ms":1234567890,"polymarket_market_start_timestamp_ms":1234567000,"polymarket_market_end_timestamp_ms":1234867000}"#,
    )
    .expect("strategy input evidence should write");
    let strategy_input_hash = Phase8OperatorApprovalEnvelope::sha256_file(&strategy_input_path)
        .expect("strategy input evidence hash should compute");

    let audit = strategy_audit_from_evidence_file(&strategy_input_path, strategy_input_hash)
        .expect("matching strategy input evidence should parse");

    assert!(!audit.is_approved());
    assert!(
        audit
            .block_reasons()
            .contains(&Phase8CanaryBlockReason::NonPositiveExpectedEdgeBasisPoints)
    );

    std::fs::write(
        &strategy_input_path,
        r#"{"realized_volatility":"2.5","seconds_to_market_end":300,"spot_price":"100000.0","price_to_beat_value":"100000.0","expected_edge_basis_points":"12.5","worst_case_edge_basis_points":"11.5","fee_rate_basis_points":"0","price_to_beat_source":"chainlink_data_streams.configured-reference-price","reference_quote_ts_event":1234567890,"pricing_kurtosis":"0","theta_decay_factor":"0","theta_scaled_min_edge_bps":"12.5","market_selection_timestamp_ms":1234567890,"market_selection_outcome":"current","polymarket_condition_id":"configured-condition","polymarket_market_slug":"configured-asset-updown-configuredwindow","polymarket_question_id":"configured-question","up_instrument_id":"configured-condition-UP.POLYMARKET","down_instrument_id":"configured-condition-DOWN.POLYMARKET","selected_market_observed_timestamp_ms":1234567890,"polymarket_market_start_timestamp_ms":1234567000,"polymarket_market_end_timestamp_ms":1234867000}"#,
    )
    .expect("strategy input evidence should write");
    let strategy_input_hash = Phase8OperatorApprovalEnvelope::sha256_file(&strategy_input_path)
        .expect("strategy input evidence hash should compute");

    let audit = strategy_audit_from_evidence_file(&strategy_input_path, strategy_input_hash)
        .expect("matching strategy input evidence should parse");

    assert!(!audit.is_approved());
    assert!(
        audit
            .block_reasons()
            .contains(&Phase8CanaryBlockReason::EdgeBasisPointsMismatch)
    );

    std::fs::write(
        &strategy_input_path,
        r#"{"realized_volatility":"2.5","seconds_to_market_end":300,"spot_price":"100000.0","price_to_beat_value":"100000.0","expected_edge_basis_points":"12.5","worst_case_edge_basis_points":"12.5","fee_rate_basis_points":"-0.1","price_to_beat_source":"chainlink_data_streams.configured-reference-price","reference_quote_ts_event":1234567890,"pricing_kurtosis":"0","theta_decay_factor":"0","theta_scaled_min_edge_bps":"12.5","market_selection_timestamp_ms":1234567890,"market_selection_outcome":"current","polymarket_condition_id":"configured-condition","polymarket_market_slug":"configured-asset-updown-configuredwindow","polymarket_question_id":"configured-question","up_instrument_id":"configured-condition-UP.POLYMARKET","down_instrument_id":"configured-condition-DOWN.POLYMARKET","selected_market_observed_timestamp_ms":1234567890,"polymarket_market_start_timestamp_ms":1234567000,"polymarket_market_end_timestamp_ms":1234867000}"#,
    )
    .expect("strategy input evidence should write");
    let strategy_input_hash = Phase8OperatorApprovalEnvelope::sha256_file(&strategy_input_path)
        .expect("strategy input evidence hash should compute");

    let audit = strategy_audit_from_evidence_file(&strategy_input_path, strategy_input_hash)
        .expect("matching strategy input evidence should parse");

    assert!(!audit.is_approved());
    assert!(
        audit
            .block_reasons()
            .contains(&Phase8CanaryBlockReason::NegativeFeeRateBasisPoints)
    );
}

#[test]
fn strategy_audit_blocks_non_positive_theta_scaled_min_edge() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let strategy_input_path = temp.path().join("phase8-strategy-input-evidence.json");
    std::fs::write(
        &strategy_input_path,
        r#"{"realized_volatility":"2.5","seconds_to_market_end":300,"spot_price":"100000.0","price_to_beat_value":"100000.0","expected_edge_basis_points":"12.5","worst_case_edge_basis_points":"12.5","fee_rate_basis_points":"0","price_to_beat_source":"chainlink_data_streams.configured-reference-price","reference_quote_ts_event":1234567890,"pricing_kurtosis":"0","theta_decay_factor":"0","theta_scaled_min_edge_bps":"-1","market_selection_timestamp_ms":1234567890,"market_selection_outcome":"current","polymarket_condition_id":"configured-condition","polymarket_market_slug":"configured-asset-updown-configuredwindow","polymarket_question_id":"configured-question","up_instrument_id":"configured-condition-UP.POLYMARKET","down_instrument_id":"configured-condition-DOWN.POLYMARKET","selected_market_observed_timestamp_ms":1234567890,"polymarket_market_start_timestamp_ms":1234567000,"polymarket_market_end_timestamp_ms":1234867000}"#,
    )
    .expect("strategy input evidence should write");
    let strategy_input_hash = Phase8OperatorApprovalEnvelope::sha256_file(&strategy_input_path)
        .expect("strategy input evidence hash should compute");

    let audit = strategy_audit_from_evidence_file(&strategy_input_path, strategy_input_hash)
        .expect("matching strategy input evidence should parse");

    assert!(!audit.is_approved());
    assert!(
        audit
            .block_reasons()
            .contains(&Phase8CanaryBlockReason::NonPositiveThetaScaledMinEdgeBps)
    );
}

#[test]
fn strategy_audit_blocks_missing_source_or_reference_timestamp() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let strategy_input_path = temp.path().join("phase8-strategy-input-evidence.json");
    std::fs::write(
        &strategy_input_path,
        r#"{"realized_volatility":"2.5","seconds_to_market_end":300,"spot_price":"100000.0","price_to_beat_value":"100000.0","expected_edge_basis_points":"12.5","worst_case_edge_basis_points":"12.5","fee_rate_basis_points":"0","price_to_beat_source":"","reference_quote_ts_event":1234567890,"pricing_kurtosis":"0","theta_decay_factor":"0","theta_scaled_min_edge_bps":"12.5","market_selection_timestamp_ms":1234567890,"market_selection_outcome":"current","polymarket_condition_id":"configured-condition","polymarket_market_slug":"configured-asset-updown-configuredwindow","polymarket_question_id":"configured-question","up_instrument_id":"configured-condition-UP.POLYMARKET","down_instrument_id":"configured-condition-DOWN.POLYMARKET","selected_market_observed_timestamp_ms":1234567890,"polymarket_market_start_timestamp_ms":1234567000,"polymarket_market_end_timestamp_ms":1234867000}"#,
    )
    .expect("strategy input evidence should write");
    let strategy_input_hash = Phase8OperatorApprovalEnvelope::sha256_file(&strategy_input_path)
        .expect("strategy input evidence hash should compute");

    let audit = strategy_audit_from_evidence_file(&strategy_input_path, strategy_input_hash)
        .expect("matching strategy input evidence should parse");

    assert!(!audit.is_approved());
    assert!(
        audit
            .block_reasons()
            .contains(&Phase8CanaryBlockReason::MissingPriceToBeatSource)
    );

    std::fs::write(
        &strategy_input_path,
        r#"{"realized_volatility":"2.5","seconds_to_market_end":300,"spot_price":"100000.0","price_to_beat_value":"100000.0","expected_edge_basis_points":"12.5","worst_case_edge_basis_points":"12.5","fee_rate_basis_points":"0","price_to_beat_source":"manual","reference_quote_ts_event":1234567890,"pricing_kurtosis":"0","theta_decay_factor":"0","theta_scaled_min_edge_bps":"12.5","market_selection_timestamp_ms":1234567890,"market_selection_outcome":"current","polymarket_condition_id":"configured-condition","polymarket_market_slug":"configured-asset-updown-configuredwindow","polymarket_question_id":"configured-question","up_instrument_id":"configured-condition-UP.POLYMARKET","down_instrument_id":"configured-condition-DOWN.POLYMARKET","selected_market_observed_timestamp_ms":1234567890,"polymarket_market_start_timestamp_ms":1234567000,"polymarket_market_end_timestamp_ms":1234867000}"#,
    )
    .expect("strategy input evidence should write");
    let strategy_input_hash = Phase8OperatorApprovalEnvelope::sha256_file(&strategy_input_path)
        .expect("strategy input evidence hash should compute");

    let audit = strategy_audit_from_evidence_file(&strategy_input_path, strategy_input_hash)
        .expect("matching strategy input evidence should parse");

    assert!(!audit.is_approved());
    assert!(
        audit
            .block_reasons()
            .contains(&Phase8CanaryBlockReason::UnsupportedPriceToBeatSource)
    );

    std::fs::write(
        &strategy_input_path,
        r#"{"realized_volatility":"2.5","seconds_to_market_end":300,"spot_price":"100000.0","price_to_beat_value":"100000.0","expected_edge_basis_points":"12.5","worst_case_edge_basis_points":"12.5","fee_rate_basis_points":"0","price_to_beat_source":"chainlink_data_streams.configured-reference-price","reference_quote_ts_event":0,"pricing_kurtosis":"0","theta_decay_factor":"0","theta_scaled_min_edge_bps":"12.5","market_selection_timestamp_ms":1234567890,"market_selection_outcome":"current","polymarket_condition_id":"configured-condition","polymarket_market_slug":"configured-asset-updown-configuredwindow","polymarket_question_id":"configured-question","up_instrument_id":"configured-condition-UP.POLYMARKET","down_instrument_id":"configured-condition-DOWN.POLYMARKET","selected_market_observed_timestamp_ms":1234567890,"polymarket_market_start_timestamp_ms":1234567000,"polymarket_market_end_timestamp_ms":1234867000}"#,
    )
    .expect("strategy input evidence should write");
    let strategy_input_hash = Phase8OperatorApprovalEnvelope::sha256_file(&strategy_input_path)
        .expect("strategy input evidence hash should compute");

    let audit = strategy_audit_from_evidence_file(&strategy_input_path, strategy_input_hash)
        .expect("matching strategy input evidence should parse");

    assert!(!audit.is_approved());
    assert!(
        audit
            .block_reasons()
            .contains(&Phase8CanaryBlockReason::MissingReferenceQuoteTsEvent)
    );
}

#[test]
fn strategy_audit_blocks_invalid_kurtosis_or_theta_inputs() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let strategy_input_path = temp.path().join("phase8-strategy-input-evidence.json");
    std::fs::write(
        &strategy_input_path,
        r#"{"realized_volatility":"2.5","seconds_to_market_end":300,"spot_price":"100000.0","price_to_beat_value":"100000.0","expected_edge_basis_points":"12.5","worst_case_edge_basis_points":"12.5","fee_rate_basis_points":"0","price_to_beat_source":"chainlink_data_streams.configured-reference-price","reference_quote_ts_event":1234567890,"pricing_kurtosis":"-6","theta_decay_factor":"0","theta_scaled_min_edge_bps":"12.5","market_selection_timestamp_ms":1234567890,"market_selection_outcome":"current","polymarket_condition_id":"configured-condition","polymarket_market_slug":"configured-asset-updown-configuredwindow","polymarket_question_id":"configured-question","up_instrument_id":"configured-condition-UP.POLYMARKET","down_instrument_id":"configured-condition-DOWN.POLYMARKET","selected_market_observed_timestamp_ms":1234567890,"polymarket_market_start_timestamp_ms":1234567000,"polymarket_market_end_timestamp_ms":1234867000}"#,
    )
    .expect("strategy input evidence should write");
    let strategy_input_hash = Phase8OperatorApprovalEnvelope::sha256_file(&strategy_input_path)
        .expect("strategy input evidence hash should compute");

    let audit = strategy_audit_from_evidence_file(&strategy_input_path, strategy_input_hash)
        .expect("matching strategy input evidence should parse");

    assert!(!audit.is_approved());
    assert!(
        audit
            .block_reasons()
            .contains(&Phase8CanaryBlockReason::InvalidPricingKurtosis)
    );

    std::fs::write(
        &strategy_input_path,
        r#"{"realized_volatility":"2.5","seconds_to_market_end":300,"spot_price":"100000.0","price_to_beat_value":"100000.0","expected_edge_basis_points":"12.5","worst_case_edge_basis_points":"12.5","fee_rate_basis_points":"0","price_to_beat_source":"chainlink_data_streams.configured-reference-price","reference_quote_ts_event":1234567890,"pricing_kurtosis":"0","theta_decay_factor":"-0.1","theta_scaled_min_edge_bps":"12.5","market_selection_timestamp_ms":1234567890,"market_selection_outcome":"current","polymarket_condition_id":"configured-condition","polymarket_market_slug":"configured-asset-updown-configuredwindow","polymarket_question_id":"configured-question","up_instrument_id":"configured-condition-UP.POLYMARKET","down_instrument_id":"configured-condition-DOWN.POLYMARKET","selected_market_observed_timestamp_ms":1234567890,"polymarket_market_start_timestamp_ms":1234567000,"polymarket_market_end_timestamp_ms":1234867000}"#,
    )
    .expect("strategy input evidence should write");
    let strategy_input_hash = Phase8OperatorApprovalEnvelope::sha256_file(&strategy_input_path)
        .expect("strategy input evidence hash should compute");

    let audit = strategy_audit_from_evidence_file(&strategy_input_path, strategy_input_hash)
        .expect("matching strategy input evidence should parse");

    assert!(!audit.is_approved());
    assert!(
        audit
            .block_reasons()
            .contains(&Phase8CanaryBlockReason::NegativeThetaDecayFactor)
    );
}

#[test]
fn strategy_audit_blocks_missing_selected_market_identity_or_window() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let strategy_input_path = temp.path().join("phase8-strategy-input-evidence.json");
    std::fs::write(
        &strategy_input_path,
        r#"{"realized_volatility":"2.5","seconds_to_market_end":300,"spot_price":"100000.0","price_to_beat_value":"100000.0","expected_edge_basis_points":"12.5","worst_case_edge_basis_points":"12.5","fee_rate_basis_points":"0","price_to_beat_source":"chainlink_data_streams.configured-reference-price","reference_quote_ts_event":1234567890,"pricing_kurtosis":"0","theta_decay_factor":"0","theta_scaled_min_edge_bps":"12.5","market_selection_timestamp_ms":1234567890,"market_selection_outcome":"current","polymarket_condition_id":"","polymarket_market_slug":"configured-asset-updown-configuredwindow","polymarket_question_id":"configured-question","up_instrument_id":"configured-condition-UP.POLYMARKET","down_instrument_id":"configured-condition-DOWN.POLYMARKET","selected_market_observed_timestamp_ms":1234567890,"polymarket_market_start_timestamp_ms":1234567000,"polymarket_market_end_timestamp_ms":1234867000}"#,
    )
    .expect("strategy input evidence should write");
    let strategy_input_hash = Phase8OperatorApprovalEnvelope::sha256_file(&strategy_input_path)
        .expect("strategy input evidence hash should compute");

    let audit = strategy_audit_from_evidence_file(&strategy_input_path, strategy_input_hash)
        .expect("matching strategy input evidence should parse");

    assert!(!audit.is_approved());
    assert!(
        audit
            .block_reasons()
            .contains(&Phase8CanaryBlockReason::MissingSelectedMarketIdentity)
    );

    std::fs::write(
        &strategy_input_path,
        r#"{"realized_volatility":"2.5","seconds_to_market_end":300,"spot_price":"100000.0","price_to_beat_value":"100000.0","expected_edge_basis_points":"12.5","worst_case_edge_basis_points":"12.5","fee_rate_basis_points":"0","price_to_beat_source":"chainlink_data_streams.configured-reference-price","reference_quote_ts_event":1234567890,"pricing_kurtosis":"0","theta_decay_factor":"0","theta_scaled_min_edge_bps":"12.5","market_selection_timestamp_ms":1234567890,"market_selection_outcome":"current","polymarket_condition_id":"configured-condition","polymarket_market_slug":"configured-asset-updown-configuredwindow","polymarket_question_id":"configured-question","up_instrument_id":"configured-condition-UP.POLYMARKET","down_instrument_id":"configured-condition-DOWN.POLYMARKET","selected_market_observed_timestamp_ms":1234567890,"polymarket_market_start_timestamp_ms":1234567000,"polymarket_market_end_timestamp_ms":1234567000}"#,
    )
    .expect("strategy input evidence should write");
    let strategy_input_hash = Phase8OperatorApprovalEnvelope::sha256_file(&strategy_input_path)
        .expect("strategy input evidence hash should compute");

    let audit = strategy_audit_from_evidence_file(&strategy_input_path, strategy_input_hash)
        .expect("matching strategy input evidence should parse");

    assert!(!audit.is_approved());
    assert!(
        audit
            .block_reasons()
            .contains(&Phase8CanaryBlockReason::InvalidSelectedMarketWindow)
    );
}

#[test]
fn strategy_audit_blocks_missing_market_selection_timestamp() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let strategy_input_path = temp.path().join("phase8-strategy-input-evidence.json");
    std::fs::write(
        &strategy_input_path,
        r#"{"realized_volatility":"2.5","seconds_to_market_end":300,"spot_price":"100000.0","price_to_beat_value":"100000.0","expected_edge_basis_points":"12.5","worst_case_edge_basis_points":"12.5","fee_rate_basis_points":"0","price_to_beat_source":"chainlink_data_streams.configured-reference-price","reference_quote_ts_event":1234567890,"pricing_kurtosis":"0","theta_decay_factor":"0","theta_scaled_min_edge_bps":"12.5","market_selection_timestamp_ms":0,"market_selection_outcome":"next","polymarket_condition_id":"configured-condition","polymarket_market_slug":"configured-asset-updown-configuredwindow","polymarket_question_id":"configured-question","up_instrument_id":"configured-condition-UP.POLYMARKET","down_instrument_id":"configured-condition-DOWN.POLYMARKET","selected_market_observed_timestamp_ms":1234567890,"polymarket_market_start_timestamp_ms":1234567000,"polymarket_market_end_timestamp_ms":1234867000}"#,
    )
    .expect("strategy input evidence should write");
    let strategy_input_hash = Phase8OperatorApprovalEnvelope::sha256_file(&strategy_input_path)
        .expect("strategy input evidence hash should compute");

    let audit = strategy_audit_from_evidence_file(&strategy_input_path, strategy_input_hash)
        .expect("matching strategy input evidence should parse");

    assert!(!audit.is_approved());
    assert!(
        audit
            .block_reasons()
            .contains(&Phase8CanaryBlockReason::InvalidSelectedMarketWindow)
    );
}

#[test]
fn strategy_audit_requires_nearest_next_market_selection() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let strategy_input_path = temp.path().join("phase8-strategy-input-evidence.json");
    std::fs::write(
        &strategy_input_path,
        r#"{"realized_volatility":"2.5","seconds_to_market_end":300,"spot_price":"100000.0","price_to_beat_value":"100000.0","expected_edge_basis_points":"12.5","worst_case_edge_basis_points":"12.5","fee_rate_basis_points":"0","price_to_beat_source":"chainlink_data_streams.configured-reference-price","reference_quote_ts_event":1234567890,"pricing_kurtosis":"0","theta_decay_factor":"0","theta_scaled_min_edge_bps":"12.5","market_selection_timestamp_ms":1234567890,"candidate_market_start_timestamps_ms":[1234667000,1234767000],"market_selection_outcome":"next","polymarket_condition_id":"configured-condition","polymarket_market_slug":"configured-asset-updown-configuredwindow","polymarket_question_id":"configured-question","up_instrument_id":"configured-condition-UP.POLYMARKET","down_instrument_id":"configured-condition-DOWN.POLYMARKET","selected_market_observed_timestamp_ms":1234567890,"polymarket_market_start_timestamp_ms":1234767000,"polymarket_market_end_timestamp_ms":1235067000}"#,
    )
    .expect("strategy input evidence should write");
    let strategy_input_hash = Phase8OperatorApprovalEnvelope::sha256_file(&strategy_input_path)
        .expect("strategy input evidence hash should compute");

    let audit = strategy_audit_from_evidence_file(&strategy_input_path, strategy_input_hash)
        .expect("matching strategy input evidence should parse");

    assert!(!audit.is_approved());
    assert!(
        audit
            .block_reasons()
            .contains(&Phase8CanaryBlockReason::InvalidMarketSelectionBinding)
    );

    let market_selection_source_path = temp.path().join("market-selection-source.json");
    std::fs::write(
        &market_selection_source_path,
        serde_json::to_vec(&serde_json::json!({
            "record_kind": "market_selection_result",
            "source": "nt_runtime_selection_snapshot",
            "market_selection_timestamp_ms": 1234567890_u64,
            "candidate_market_start_timestamps_ms": [1234667000_u64, 1234767000_u64],
            "market_selection_outcome": "next",
            "polymarket_condition_id": "configured-condition",
            "polymarket_market_slug": "configured-asset-updown-configuredwindow",
            "polymarket_question_id": "configured-question",
            "up_instrument_id": "configured-condition-UP.POLYMARKET",
            "down_instrument_id": "configured-condition-DOWN.POLYMARKET",
            "selected_market_observed_timestamp_ms": 1234567890_u64,
            "polymarket_market_start_timestamp_ms": 1234667000_u64,
            "polymarket_market_end_timestamp_ms": 1234967000_u64
        }))
        .expect("market selection source evidence should serialize"),
    )
    .expect("market selection source evidence should write");
    let market_selection_source_hash =
        Phase8OperatorApprovalEnvelope::sha256_file(&market_selection_source_path)
            .expect("market selection source evidence hash should compute");
    std::fs::write(
        &strategy_input_path,
        serde_json::to_vec(&serde_json::json!({
            "realized_volatility": "2.5",
            "seconds_to_market_end": 300_u64,
            "spot_price": "100000.0",
            "price_to_beat_value": "100000.0",
            "expected_edge_basis_points": "12.5",
            "worst_case_edge_basis_points": "12.5",
            "fee_rate_basis_points": "0",
            "price_to_beat_source": "chainlink_data_streams.configured-reference-price",
            "gate_session_hash": PHASE8_TEST_GATE_SESSION_HASH,
            "selected_market_key": PHASE8_TEST_SELECTED_MARKET_KEY,
            "gate_evidence": {
                "resolution": {
                    "satisfaction_kind": "evidence",
                    "selected_market_key": PHASE8_TEST_SELECTED_MARKET_KEY,
                    "provider_id": "chainlink_main",
                    "provider_kind": "chainlink_data_streams",
                    "value_kind": "scalar_price",
                    "normalized_value_sha256": PHASE8_TEST_GATE_NORMALIZED_VALUE_HASH,
                    "provider_provenance_sha256": PHASE8_TEST_GATE_PROVIDER_PROVENANCE_HASH,
                    "artifact_sha256s": [PHASE8_TEST_GATE_ARTIFACT_HASH]
                }
            },
            "reference_quote_ts_event": 1234567890_u64,
            "pricing_kurtosis": "0",
            "theta_decay_factor": "0",
            "theta_scaled_min_edge_bps": "12.5",
            "market_selection_timestamp_ms": 1234567890_u64,
            "candidate_market_start_timestamps_ms": [1234667000_u64, 1234767000_u64],
            "market_selection_source_path": market_selection_source_path.to_string_lossy(),
            "market_selection_source_sha256": market_selection_source_hash,
            "market_selection_outcome": "next",
            "polymarket_condition_id": "configured-condition",
            "polymarket_market_slug": "configured-asset-updown-configuredwindow",
            "polymarket_question_id": "configured-question",
            "up_instrument_id": "configured-condition-UP.POLYMARKET",
            "down_instrument_id": "configured-condition-DOWN.POLYMARKET",
            "selected_market_observed_timestamp_ms": 1234567890_u64,
            "polymarket_market_start_timestamp_ms": 1234667000_u64,
            "polymarket_market_end_timestamp_ms": 1234967000_u64
        }))
        .expect("strategy input evidence should serialize"),
    )
    .expect("strategy input evidence should write");
    let strategy_input_hash = Phase8OperatorApprovalEnvelope::sha256_file(&strategy_input_path)
        .expect("strategy input evidence hash should compute");

    let audit = strategy_audit_from_evidence_file(&strategy_input_path, strategy_input_hash)
        .expect("matching strategy input evidence should parse");

    assert!(audit.is_approved());
}

#[test]
fn strategy_audit_requires_source_bound_current_market_selection() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let strategy_input_path = temp.path().join("phase8-strategy-input-evidence.json");
    std::fs::write(
        &strategy_input_path,
        r#"{"realized_volatility":"2.5","seconds_to_market_end":300,"spot_price":"100000.0","price_to_beat_value":"100000.0","expected_edge_basis_points":"12.5","worst_case_edge_basis_points":"12.5","fee_rate_basis_points":"0","price_to_beat_source":"chainlink_data_streams.configured-reference-price","reference_quote_ts_event":1234567890,"pricing_kurtosis":"0","theta_decay_factor":"0","theta_scaled_min_edge_bps":"12.5","market_selection_timestamp_ms":1234567890,"market_selection_outcome":"current","polymarket_condition_id":"configured-condition","polymarket_market_slug":"configured-asset-updown-configuredwindow","polymarket_question_id":"configured-question","up_instrument_id":"configured-condition-UP.POLYMARKET","down_instrument_id":"configured-condition-DOWN.POLYMARKET","selected_market_observed_timestamp_ms":1234567890,"polymarket_market_start_timestamp_ms":1234567000,"polymarket_market_end_timestamp_ms":1234867000}"#,
    )
    .expect("strategy input evidence should write");
    let strategy_input_hash = Phase8OperatorApprovalEnvelope::sha256_file(&strategy_input_path)
        .expect("strategy input evidence hash should compute");

    let audit = strategy_audit_from_evidence_file(&strategy_input_path, strategy_input_hash)
        .expect("current strategy input evidence should parse");

    assert!(!audit.is_approved());
    assert!(
        audit
            .block_reasons()
            .contains(&Phase8CanaryBlockReason::InvalidMarketSelectionBinding)
    );
}

#[test]
fn strategy_audit_rejects_next_market_without_source_bound_candidates() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let strategy_input_path = temp.path().join("phase8-strategy-input-evidence.json");
    std::fs::write(
        &strategy_input_path,
        r#"{"realized_volatility":"2.5","seconds_to_market_end":300,"spot_price":"100000.0","price_to_beat_value":"100000.0","expected_edge_basis_points":"12.5","worst_case_edge_basis_points":"12.5","fee_rate_basis_points":"0","price_to_beat_source":"chainlink_data_streams.configured-reference-price","reference_quote_ts_event":1234567890,"pricing_kurtosis":"0","theta_decay_factor":"0","theta_scaled_min_edge_bps":"12.5","market_selection_timestamp_ms":1234567890,"candidate_market_start_timestamps_ms":[1234767000],"market_selection_outcome":"next","polymarket_condition_id":"configured-condition","polymarket_market_slug":"configured-asset-updown-configuredwindow","polymarket_question_id":"configured-question","up_instrument_id":"configured-condition-UP.POLYMARKET","down_instrument_id":"configured-condition-DOWN.POLYMARKET","selected_market_observed_timestamp_ms":1234567890,"polymarket_market_start_timestamp_ms":1234767000,"polymarket_market_end_timestamp_ms":1235067000}"#,
    )
    .expect("strategy input evidence should write");
    let strategy_input_hash = Phase8OperatorApprovalEnvelope::sha256_file(&strategy_input_path)
        .expect("strategy input evidence hash should compute");

    let audit = strategy_audit_from_evidence_file(&strategy_input_path, strategy_input_hash)
        .expect("matching strategy input evidence should parse");

    assert!(!audit.is_approved());
    assert!(
        audit
            .block_reasons()
            .contains(&Phase8CanaryBlockReason::InvalidMarketSelectionBinding)
    );
}

#[test]
fn strategy_audit_rejects_next_market_candidate_list_truncated_from_source() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let market_selection_source_path = temp.path().join("market-selection-source.json");
    std::fs::write(
        &market_selection_source_path,
        serde_json::to_vec(&serde_json::json!({
            "record_kind": "market_selection_result",
            "source": "nt_runtime_selection_snapshot",
            "market_selection_timestamp_ms": 1234567890_u64,
            "candidate_market_start_timestamps_ms": [1234667000_u64, 1234767000_u64],
            "market_selection_outcome": "next",
            "polymarket_condition_id": "configured-condition",
            "polymarket_market_slug": "configured-asset-updown-configuredwindow",
            "polymarket_question_id": "configured-question",
            "up_instrument_id": "configured-condition-UP.POLYMARKET",
            "down_instrument_id": "configured-condition-DOWN.POLYMARKET",
            "selected_market_observed_timestamp_ms": 1234567890_u64,
            "polymarket_market_start_timestamp_ms": 1234767000_u64,
            "polymarket_market_end_timestamp_ms": 1235067000_u64
        }))
        .expect("market selection source evidence should serialize"),
    )
    .expect("market selection source evidence should write");
    let market_selection_source_hash =
        Phase8OperatorApprovalEnvelope::sha256_file(&market_selection_source_path)
            .expect("market selection source evidence hash should compute");
    let strategy_input_path = temp.path().join("phase8-strategy-input-evidence.json");
    std::fs::write(
        &strategy_input_path,
        serde_json::to_vec(&serde_json::json!({
            "realized_volatility": "2.5",
            "seconds_to_market_end": 300_u64,
            "spot_price": "100000.0",
            "price_to_beat_value": "100000.0",
            "expected_edge_basis_points": "12.5",
            "worst_case_edge_basis_points": "12.5",
            "fee_rate_basis_points": "0",
            "price_to_beat_source": "chainlink_data_streams.configured-reference-price",
            "reference_quote_ts_event": 1234567890_u64,
            "pricing_kurtosis": "0",
            "theta_decay_factor": "0",
            "theta_scaled_min_edge_bps": "12.5",
            "market_selection_timestamp_ms": 1234567890_u64,
            "candidate_market_start_timestamps_ms": [1234767000_u64],
            "market_selection_source_path": market_selection_source_path.to_string_lossy(),
            "market_selection_source_sha256": market_selection_source_hash,
            "market_selection_outcome": "next",
            "polymarket_condition_id": "configured-condition",
            "polymarket_market_slug": "configured-asset-updown-configuredwindow",
            "polymarket_question_id": "configured-question",
            "up_instrument_id": "configured-condition-UP.POLYMARKET",
            "down_instrument_id": "configured-condition-DOWN.POLYMARKET",
            "selected_market_observed_timestamp_ms": 1234567890_u64,
            "polymarket_market_start_timestamp_ms": 1234767000_u64,
            "polymarket_market_end_timestamp_ms": 1235067000_u64
        }))
        .expect("strategy input evidence should serialize"),
    )
    .expect("strategy input evidence should write");
    let strategy_input_hash = Phase8OperatorApprovalEnvelope::sha256_file(&strategy_input_path)
        .expect("strategy input evidence hash should compute");

    let audit = strategy_audit_from_evidence_file(&strategy_input_path, strategy_input_hash)
        .expect("matching strategy input evidence should parse");

    assert!(!audit.is_approved());
    assert!(
        audit
            .block_reasons()
            .contains(&Phase8CanaryBlockReason::InvalidMarketSelectionBinding)
    );
}

#[test]
fn strategy_audit_blocks_invalid_market_selection_outcome() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let strategy_input_path = temp.path().join("phase8-strategy-input-evidence.json");
    std::fs::write(
        &strategy_input_path,
        r#"{"realized_volatility":"2.5","seconds_to_market_end":300,"spot_price":"100000.0","price_to_beat_value":"100000.0","expected_edge_basis_points":"12.5","worst_case_edge_basis_points":"12.5","fee_rate_basis_points":"0","price_to_beat_source":"chainlink_data_streams.configured-reference-price","reference_quote_ts_event":1234567890,"pricing_kurtosis":"0","theta_decay_factor":"0","theta_scaled_min_edge_bps":"12.5","market_selection_timestamp_ms":1234567890,"market_selection_outcome":"failed","polymarket_condition_id":"configured-condition","polymarket_market_slug":"configured-asset-updown-configuredwindow","polymarket_question_id":"configured-question","up_instrument_id":"configured-condition-UP.POLYMARKET","down_instrument_id":"configured-condition-DOWN.POLYMARKET","selected_market_observed_timestamp_ms":1234567890,"polymarket_market_start_timestamp_ms":1234567000,"polymarket_market_end_timestamp_ms":1234867000}"#,
    )
    .expect("strategy input evidence should write");
    let strategy_input_hash = Phase8OperatorApprovalEnvelope::sha256_file(&strategy_input_path)
        .expect("strategy input evidence hash should compute");

    let audit = strategy_audit_from_evidence_file(&strategy_input_path, strategy_input_hash)
        .expect("matching strategy input evidence should parse");

    assert!(!audit.is_approved());
    assert!(
        audit
            .block_reasons()
            .contains(&Phase8CanaryBlockReason::InvalidMarketSelectionOutcome)
    );
    assert!(
        audit
            .block_reasons()
            .contains(&Phase8CanaryBlockReason::InvalidMarketSelectionBinding)
    );
}

#[test]
fn strategy_audit_blocks_market_selection_window_mismatch() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let strategy_input_path = temp.path().join("phase8-strategy-input-evidence.json");
    std::fs::write(
        &strategy_input_path,
        r#"{"realized_volatility":"2.5","seconds_to_market_end":300,"spot_price":"100000.0","price_to_beat_value":"100000.0","expected_edge_basis_points":"12.5","worst_case_edge_basis_points":"12.5","fee_rate_basis_points":"0","price_to_beat_source":"chainlink_data_streams.configured-reference-price","reference_quote_ts_event":1234567890,"pricing_kurtosis":"0","theta_decay_factor":"0","theta_scaled_min_edge_bps":"12.5","market_selection_timestamp_ms":1235000000,"market_selection_outcome":"current","polymarket_condition_id":"configured-condition","polymarket_market_slug":"configured-asset-updown-configuredwindow","polymarket_question_id":"configured-question","up_instrument_id":"configured-condition-UP.POLYMARKET","down_instrument_id":"configured-condition-DOWN.POLYMARKET","selected_market_observed_timestamp_ms":1234567890,"polymarket_market_start_timestamp_ms":1234567000,"polymarket_market_end_timestamp_ms":1234867000}"#,
    )
    .expect("strategy input evidence should write");
    let strategy_input_hash = Phase8OperatorApprovalEnvelope::sha256_file(&strategy_input_path)
        .expect("strategy input evidence hash should compute");

    let audit = strategy_audit_from_evidence_file(&strategy_input_path, strategy_input_hash)
        .expect("matching strategy input evidence should parse");

    assert!(!audit.is_approved());
    assert!(
        audit
            .block_reasons()
            .contains(&Phase8CanaryBlockReason::InvalidMarketSelectionBinding)
    );

    std::fs::write(
        &strategy_input_path,
        r#"{"realized_volatility":"2.5","seconds_to_market_end":300,"spot_price":"100000.0","price_to_beat_value":"100000.0","expected_edge_basis_points":"12.5","worst_case_edge_basis_points":"12.5","fee_rate_basis_points":"0","price_to_beat_source":"chainlink_data_streams.configured-reference-price","reference_quote_ts_event":1234567890,"pricing_kurtosis":"0","theta_decay_factor":"0","theta_scaled_min_edge_bps":"12.5","market_selection_timestamp_ms":1234567890,"market_selection_outcome":"next","polymarket_condition_id":"configured-condition","polymarket_market_slug":"configured-asset-updown-configuredwindow","polymarket_question_id":"configured-question","up_instrument_id":"configured-condition-UP.POLYMARKET","down_instrument_id":"configured-condition-DOWN.POLYMARKET","selected_market_observed_timestamp_ms":1234567890,"polymarket_market_start_timestamp_ms":1234567000,"polymarket_market_end_timestamp_ms":1234867000}"#,
    )
    .expect("strategy input evidence should write");
    let strategy_input_hash = Phase8OperatorApprovalEnvelope::sha256_file(&strategy_input_path)
        .expect("strategy input evidence hash should compute");

    let audit = strategy_audit_from_evidence_file(&strategy_input_path, strategy_input_hash)
        .expect("matching strategy input evidence should parse");

    assert!(!audit.is_approved());
    assert!(
        audit
            .block_reasons()
            .contains(&Phase8CanaryBlockReason::InvalidMarketSelectionBinding)
    );
}

#[test]
fn strategy_audit_rejects_unknown_input_evidence_fields() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let evidence_path = temp.path().join("strategy-input-evidence.json");
    std::fs::write(
        &evidence_path,
        r#"{"realized_volatility":"2.5","seconds_to_market_end":300,"spot_price":"100000.0","price_to_beat_value":"100000.0","expected_edge_basis_points":"12.5","worst_case_edge_basis_points":"12.5","fee_rate_basis_points":"0","price_to_beat_source":"chainlink_data_streams.configured-reference-price","reference_quote_ts_event":1234567890,"pricing_kurtosis":"0","theta_decay_factor":"0","theta_scaled_min_edge_bps":"12.5","market_selection_timestamp_ms":1234567890,"market_selection_outcome":"current","polymarket_condition_id":"configured-condition","polymarket_market_slug":"configured-asset-updown-configuredwindow","polymarket_question_id":"configured-question","up_instrument_id":"configured-condition-UP.POLYMARKET","down_instrument_id":"configured-condition-DOWN.POLYMARKET","selected_market_observed_timestamp_ms":1234567890,"polymarket_market_start_timestamp_ms":1234567000,"polymarket_market_end_timestamp_ms":1234867000,"unreviewed_override":"accepted"}"#,
    )
    .expect("strategy input evidence should write");
    let evidence_hash = Phase8OperatorApprovalEnvelope::sha256_file(&evidence_path)
        .expect("strategy input evidence hash should compute");

    let error = strategy_audit_from_evidence_file(&evidence_path, &evidence_hash)
        .expect_err("unknown strategy input evidence fields should fail");

    assert!(
        error.to_string().contains("unknown field"),
        "error should mention unknown strategy input evidence field: {error}"
    );
}

#[test]
fn strategy_audit_verifies_input_evidence_hash_before_approving() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let evidence_path = temp.path().join("strategy-input-evidence.json");
    let (source_path, source_hash) =
        write_current_market_selection_source(temp.path()).expect("source should write");
    write_current_strategy_input_evidence(
        &evidence_path,
        PHASE8_TEST_PRICE_TO_BEAT_SOURCE,
        &source_path,
        &source_hash,
    )
    .expect("strategy input evidence should write");
    let evidence_hash = Phase8OperatorApprovalEnvelope::sha256_file(&evidence_path)
        .expect("strategy input evidence hash should compute");

    let audit = strategy_audit_from_evidence_file(&evidence_path, &evidence_hash)
        .expect("matching strategy input evidence should parse");

    assert!(audit.is_approved());
}

#[test]
fn strategy_audit_rejects_input_evidence_hash_mismatch() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let evidence_path = temp.path().join("strategy-input-evidence.json");
    std::fs::write(
        &evidence_path,
        r#"{"realized_volatility":"2.5","seconds_to_market_end":300,"spot_price":"100000.0","price_to_beat_value":"100000.0","expected_edge_basis_points":"12.5","worst_case_edge_basis_points":"12.5","fee_rate_basis_points":"0","price_to_beat_source":"chainlink_data_streams.configured-reference-price","reference_quote_ts_event":1234567890,"pricing_kurtosis":"0","theta_decay_factor":"0","theta_scaled_min_edge_bps":"12.5","market_selection_timestamp_ms":1234567890,"market_selection_outcome":"current","polymarket_condition_id":"configured-condition","polymarket_market_slug":"configured-asset-updown-configuredwindow","polymarket_question_id":"configured-question","up_instrument_id":"configured-condition-UP.POLYMARKET","down_instrument_id":"configured-condition-DOWN.POLYMARKET","selected_market_observed_timestamp_ms":1234567890,"polymarket_market_start_timestamp_ms":1234567000,"polymarket_market_end_timestamp_ms":1234867000}"#,
    )
    .expect("strategy input evidence should write");

    let error = strategy_audit_from_evidence_file(&evidence_path, "wrong-hash")
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
fn dry_canary_evidence_writer_rejects_malformed_ref_hashes() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let evidence_path = temp.path().join("phase8-canary-evidence.json");
    let evidence = Phase8CanaryEvidence::dry_no_submit_proof(
        evidence_input(),
        Phase8EvidenceRef {
            path_hash: "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
                .to_string(),
            record_hash: "not-a-sha256".to_string(),
        },
    );

    let error = evidence
        .write_json_file(&evidence_path)
        .expect_err("malformed dry proof ref must not be written");

    assert!(
        error
            .to_string()
            .contains("decision_evidence_ref.record_hash"),
        "error should mention malformed dry proof decision ref: {error}"
    );
    assert!(
        !evidence_path.exists(),
        "malformed dry proof must not create evidence file"
    );
}

#[test]
fn dry_canary_evidence_writer_rejects_live_order_ref() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let evidence_path = temp.path().join("phase8-canary-evidence.json");
    let mut evidence = Phase8CanaryEvidence::dry_no_submit_proof(
        evidence_input(),
        valid_evidence_ref("cccc", "dddd"),
    );
    evidence.live_order_ref = Some(valid_live_order_ref());

    let error = evidence
        .write_json_file(&evidence_path)
        .expect_err("dry proof must not carry live order evidence");

    assert!(
        error.to_string().contains("live_order_ref"),
        "error should mention live-only ref on dry proof: {error}"
    );
    assert!(
        !evidence_path.exists(),
        "dry proof with live-only ref must not create evidence file"
    );
}

#[test]
fn dry_canary_evidence_writer_rejects_missing_block_reason() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let evidence_path = temp.path().join("phase8-canary-evidence.json");
    let mut evidence = Phase8CanaryEvidence::dry_no_submit_proof(
        evidence_input(),
        valid_evidence_ref("cccc", "dddd"),
    );
    evidence.block_reasons.clear();

    let error = evidence
        .write_json_file(&evidence_path)
        .expect_err("dry proof must carry blocked-before-live-order reason");

    assert!(
        error.to_string().contains("block_reasons"),
        "error should mention dry proof block reasons: {error}"
    );
    assert!(
        !evidence_path.exists(),
        "dry proof without block reason must not create evidence file"
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
        vec![Phase8CanaryBlockReason::RootConfigHashUnavailable],
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
        vec![Phase8CanaryBlockReason::DecisionEvidenceUnavailable],
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
fn blocked_before_submit_preserves_all_preflight_block_reasons() {
    let block_reasons = vec![
        Phase8CanaryBlockReason::DecisionEvidenceUnavailable,
        Phase8CanaryBlockReason::RootConfigHashUnavailable,
        Phase8CanaryBlockReason::LiveCanaryGateRejected,
    ];

    let evidence =
        Phase8CanaryEvidence::blocked_before_submit(evidence_input(), block_reasons.clone());

    assert_eq!(evidence.outcome, Phase8CanaryOutcome::BlockedBeforeSubmit);
    assert_eq!(evidence.block_reasons, block_reasons);
    assert!(evidence.decision_evidence_ref.is_none());
}

#[test]
fn blocked_canary_evidence_writer_rejects_inconsistent_submit_admission() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let evidence_path = temp.path().join("phase8-canary-evidence.json");
    let mut evidence = Phase8CanaryEvidence::blocked_before_submit(
        evidence_input(),
        vec![Phase8CanaryBlockReason::DecisionEvidenceUnavailable],
    );
    evidence.submit_admission_ref.admitted_order_count = 1;

    let error = evidence
        .write_json_file(&evidence_path)
        .expect_err("blocked evidence with accepted submit count must not be written");

    assert!(
        error
            .to_string()
            .contains("submit_admission_ref.admitted_order_count"),
        "error should mention inconsistent submit admission count: {error}"
    );
    assert!(
        !evidence_path.exists(),
        "inconsistent blocked evidence must not create evidence file"
    );
}

#[test]
fn blocked_canary_evidence_writer_rejects_decision_evidence_ref() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let evidence_path = temp.path().join("phase8-canary-evidence.json");
    let mut evidence = Phase8CanaryEvidence::blocked_before_submit(
        evidence_input(),
        vec![Phase8CanaryBlockReason::DecisionEvidenceUnavailable],
    );
    evidence.decision_evidence_ref = Some(valid_evidence_ref("cccc", "dddd"));

    let error = evidence
        .write_json_file(&evidence_path)
        .expect_err("blocked evidence must not carry decision evidence");

    assert!(
        error.to_string().contains("decision_evidence_ref"),
        "error should mention decision evidence ref on blocked proof: {error}"
    );
    assert!(
        !evidence_path.exists(),
        "blocked proof with decision ref must not create evidence file"
    );
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
            strategy_instance_id_hash:
                "1212121212121212121212121212121212121212121212121212121212121212".to_string(),
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
fn live_canary_evidence_writer_rejects_block_reasons() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let evidence_path = temp.path().join("phase8-canary-evidence.json");
    let mut evidence = Phase8CanaryEvidence::live_canary_proof(
        evidence_input(),
        valid_evidence_ref("cccc", "dddd"),
        valid_live_order_ref(),
        valid_live_canary_result_refs(),
        1,
    )
    .expect("valid live canary evidence should construct");
    evidence
        .block_reasons
        .push(Phase8CanaryBlockReason::DecisionEvidenceUnavailable);

    let error = evidence
        .write_json_file(&evidence_path)
        .expect_err("live proof with block reasons must not be written");

    assert!(
        error.to_string().contains("block_reasons"),
        "error should mention live proof block reasons: {error}"
    );
    assert!(
        !evidence_path.exists(),
        "live proof with block reasons must not create evidence file"
    );
}

#[test]
fn live_canary_evidence_writer_rejects_mutated_strategy_hash() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let evidence_path = temp.path().join("phase8-canary-evidence.json");
    let mut evidence = Phase8CanaryEvidence::live_canary_proof(
        evidence_input(),
        valid_evidence_ref("cccc", "dddd"),
        valid_live_order_ref(),
        valid_live_canary_result_refs(),
        1,
    )
    .expect("valid live canary evidence should construct");
    evidence
        .live_order_ref
        .as_mut()
        .expect("live order ref should exist")
        .strategy_instance_id_hash =
        "3434343434343434343434343434343434343434343434343434343434343434".to_string();

    let error = evidence
        .write_json_file(&evidence_path)
        .expect_err("mutated live proof strategy hash must not be written");

    assert!(
        error.to_string().contains("strategy_instance_id_hash"),
        "error should mention mismatched strategy id hash: {error}"
    );
    assert!(
        !evidence_path.exists(),
        "mutated live proof strategy hash must not create evidence file"
    );
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
            strategy_instance_id_hash:
                "1212121212121212121212121212121212121212121212121212121212121212".to_string(),
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
fn live_canary_evidence_rejects_malformed_result_refs() {
    let error = Phase8CanaryEvidence::live_canary_proof(
        evidence_input(),
        Phase8EvidenceRef {
            path_hash: "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
                .to_string(),
            record_hash: "not-a-sha256".to_string(),
        },
        valid_live_order_ref(),
        valid_live_canary_result_refs(),
        1,
    )
    .expect_err("malformed decision evidence ref must not produce live canary proof");

    assert!(
        error
            .to_string()
            .contains("decision_evidence_ref.record_hash"),
        "error should mention malformed decision evidence record hash: {error}"
    );

    let error = Phase8CanaryEvidence::live_canary_proof(
        evidence_input(),
        valid_evidence_ref("cccc", "dddd"),
        Phase8LiveOrderRef {
            strategy_instance_id_hash:
                "1212121212121212121212121212121212121212121212121212121212121212".to_string(),
            client_order_id_hash: String::new(),
            venue_order_id_hash: "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"
                .to_string(),
        },
        valid_live_canary_result_refs(),
        1,
    )
    .expect_err("missing live order client hash must not produce live canary proof");

    assert!(
        error
            .to_string()
            .contains("live_order_ref.client_order_id_hash"),
        "error should mention malformed live order client hash: {error}"
    );
}

#[test]
fn live_canary_evidence_rejects_order_from_unapproved_strategy() {
    let mut live_order_ref = valid_live_order_ref();
    live_order_ref.strategy_instance_id_hash =
        "3434343434343434343434343434343434343434343434343434343434343434".to_string();

    let error = Phase8CanaryEvidence::live_canary_proof(
        evidence_input(),
        valid_evidence_ref("cccc", "dddd"),
        live_order_ref,
        valid_live_canary_result_refs(),
        1,
    )
    .expect_err("live order from an unapproved strategy must not produce canary proof");

    assert!(
        error.to_string().contains("strategy_instance_id_hash"),
        "error should mention mismatched strategy id hash: {error}"
    );
}

#[test]
fn live_canary_evidence_rejects_malformed_identity_hashes() {
    let mut input = evidence_input();
    input.root_config_sha256 = "not-a-sha256".to_string();
    let error = Phase8CanaryEvidence::live_canary_proof(
        input,
        valid_evidence_ref("cccc", "dddd"),
        valid_live_order_ref(),
        valid_live_canary_result_refs(),
        1,
    )
    .expect_err("malformed root config hash must not produce live canary proof");

    assert!(
        error.to_string().contains("root_config_sha256"),
        "error should mention malformed root config hash: {error}"
    );

    let mut input = evidence_input();
    input.runtime_capture_ref.spool_root_hash = String::new();
    let error = Phase8CanaryEvidence::live_canary_proof(
        input,
        valid_evidence_ref("cccc", "dddd"),
        valid_live_order_ref(),
        valid_live_canary_result_refs(),
        1,
    )
    .expect_err("missing runtime capture spool hash must not produce live canary proof");

    assert!(
        error
            .to_string()
            .contains("runtime_capture_ref.spool_root_hash"),
        "error should mention malformed runtime capture spool hash: {error}"
    );
}

#[test]
fn live_canary_evidence_rejects_invalid_cap_values() {
    let mut input = evidence_input();
    input.max_live_order_count = 2;
    let error = Phase8CanaryEvidence::live_canary_proof(
        input,
        valid_evidence_ref("cccc", "dddd"),
        valid_live_order_ref(),
        valid_live_canary_result_refs(),
        1,
    )
    .expect_err("non-one live order cap must not produce live canary proof");

    assert!(
        error.to_string().contains("max_live_order_count"),
        "error should mention invalid live order cap: {error}"
    );

    let mut input = evidence_input();
    input.max_notional_per_order = Decimal::ZERO;
    let error = Phase8CanaryEvidence::live_canary_proof(
        input,
        valid_evidence_ref("cccc", "dddd"),
        valid_live_order_ref(),
        valid_live_canary_result_refs(),
        1,
    )
    .expect_err("non-positive notional cap must not produce live canary proof");

    assert!(
        error.to_string().contains("max_notional_per_order"),
        "error should mention invalid notional cap: {error}"
    );
}

#[test]
fn canary_evidence_writer_rejects_mutated_cap_values() {
    let temp = tempfile::tempdir().expect("tempdir should create");

    let mut order_count_evidence = Phase8CanaryEvidence::dry_no_submit_proof(
        evidence_input(),
        valid_evidence_ref("cccc", "dddd"),
    );
    order_count_evidence.max_live_order_count = 2;
    let order_count_path = temp.path().join("phase8-canary-order-count.json");
    let error = order_count_evidence
        .write_json_file(&order_count_path)
        .expect_err("mutated live order cap must not be written");

    assert!(
        error.to_string().contains("max_live_order_count"),
        "error should mention invalid live order cap: {error}"
    );
    assert!(
        !order_count_path.exists(),
        "mutated order cap evidence must not create evidence file"
    );

    let mut notional_evidence = Phase8CanaryEvidence::dry_no_submit_proof(
        evidence_input(),
        valid_evidence_ref("cccc", "dddd"),
    );
    notional_evidence.max_notional_per_order = Decimal::ZERO.to_string();
    let notional_path = temp.path().join("phase8-canary-notional.json");
    let error = notional_evidence
        .write_json_file(&notional_path)
        .expect_err("mutated non-positive notional cap must not be written");

    assert!(
        error.to_string().contains("max_notional_per_order"),
        "error should mention invalid notional cap: {error}"
    );
    assert!(
        !notional_path.exists(),
        "mutated notional evidence must not create evidence file"
    );
}

#[test]
fn canary_evidence_writer_rejects_mutated_identity_fields() {
    let temp = tempfile::tempdir().expect("tempdir should create");

    let mut schema_evidence = Phase8CanaryEvidence::dry_no_submit_proof(
        evidence_input(),
        valid_evidence_ref("cccc", "dddd"),
    );
    schema_evidence.schema_version = 0;
    let schema_path = temp.path().join("phase8-canary-schema.json");
    let error = schema_evidence
        .write_json_file(&schema_path)
        .expect_err("mutated schema version must not be written");

    assert!(
        error.to_string().contains("schema_version"),
        "error should mention schema version: {error}"
    );
    assert!(
        !schema_path.exists(),
        "mutated schema evidence must not create evidence file"
    );

    let mut head_evidence = Phase8CanaryEvidence::dry_no_submit_proof(
        evidence_input(),
        valid_evidence_ref("cccc", "dddd"),
    );
    head_evidence.head_sha.clear();
    let head_path = temp.path().join("phase8-canary-head.json");
    let error = head_evidence
        .write_json_file(&head_path)
        .expect_err("empty head sha must not be written");

    assert!(
        error.to_string().contains("head_sha"),
        "error should mention head sha: {error}"
    );
    assert!(
        !head_path.exists(),
        "empty head evidence must not create evidence file"
    );

    let mut approval_evidence = Phase8CanaryEvidence::dry_no_submit_proof(
        evidence_input(),
        valid_evidence_ref("cccc", "dddd"),
    );
    approval_evidence.approval_id_hash = "operator-approved-canary-001".to_string();
    let approval_path = temp.path().join("phase8-canary-approval.json");
    let error = approval_evidence
        .write_json_file(&approval_path)
        .expect_err("raw approval id must not be written as approval hash");

    assert!(
        error.to_string().contains("approval_id_hash"),
        "error should mention approval id hash: {error}"
    );
    assert!(
        !approval_path.exists(),
        "raw approval evidence must not create evidence file"
    );
}

#[test]
fn canary_evidence_writer_rejects_invalid_runtime_metadata() {
    let temp = tempfile::tempdir().expect("tempdir should create");

    let mut run_id_evidence = Phase8CanaryEvidence::dry_no_submit_proof(
        evidence_input(),
        valid_evidence_ref("cccc", "dddd"),
    );
    run_id_evidence.runtime_capture_ref.run_id.clear();
    let run_id_path = temp.path().join("phase8-canary-run-id.json");
    let error = run_id_evidence
        .write_json_file(&run_id_path)
        .expect_err("empty runtime capture run id must not be written");

    assert!(
        error.to_string().contains("runtime_capture_ref.run_id"),
        "error should mention runtime capture run id: {error}"
    );
    assert!(
        !run_id_path.exists(),
        "empty runtime capture run id must not create evidence file"
    );

    let mut lifecycle_evidence = Phase8CanaryEvidence::dry_no_submit_proof(
        evidence_input(),
        valid_evidence_ref("cccc", "dddd"),
    );
    lifecycle_evidence
        .nt_lifecycle_refs
        .push(Phase8NtLifecycleRef {
            kind: String::new(),
            event_hash: "not-a-sha256".to_string(),
        });
    let lifecycle_path = temp.path().join("phase8-canary-lifecycle.json");
    let error = lifecycle_evidence
        .write_json_file(&lifecycle_path)
        .expect_err("invalid NT lifecycle ref must not be written");

    assert!(
        error.to_string().contains("nt_lifecycle_refs"),
        "error should mention NT lifecycle refs: {error}"
    );
    assert!(
        !lifecycle_path.exists(),
        "invalid NT lifecycle ref must not create evidence file"
    );
}

#[test]
fn operator_approval_envelope_rejects_head_or_checksum_mismatch() {
    let envelope = Phase8OperatorApprovalEnvelope {
        head_sha: "expected-head".to_string(),
        root_toml_path: "config/live.local.toml".to_string(),
        root_toml_sha256: "expected-config-hash".to_string(),
        approval_envelope_sha256: PHASE8_TEST_APPROVAL_ENVELOPE_SHA256.to_string(),
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
        approval_not_before_unix_secs: 1_000,
        approval_not_after_unix_secs: 2_000,
        approval_nonce_path: "phase8-approval-nonce.json".to_string(),
        approval_nonce_sha256: "expected-approval-nonce-hash".to_string(),
        approval_consumption_path: "phase8-approval-consumed.json".to_string(),
        canary_evidence_path: "phase8-canary-evidence.json".to_string(),
        strategy_cancel_path: None,
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
        r#"{"realized_volatility":"2.5","seconds_to_market_end":300,"spot_price":"100000.0","price_to_beat_value":"100000.0","expected_edge_basis_points":"12.5","worst_case_edge_basis_points":"12.5","fee_rate_basis_points":"0","price_to_beat_source":"chainlink_data_streams.configured-reference-price","reference_quote_ts_event":1234567890,"pricing_kurtosis":"0","theta_decay_factor":"0","theta_scaled_min_edge_bps":"12.5","market_selection_timestamp_ms":1234567890,"market_selection_outcome":"current","polymarket_condition_id":"configured-condition","polymarket_market_slug":"configured-asset-updown-configuredwindow","polymarket_question_id":"configured-question","up_instrument_id":"configured-condition-UP.POLYMARKET","down_instrument_id":"configured-condition-DOWN.POLYMARKET","selected_market_observed_timestamp_ms":1234567890,"polymarket_market_start_timestamp_ms":1234567000,"polymarket_market_end_timestamp_ms":1234867000}"#,
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
    let mut loaded = loaded_with_live_canary("reports/no-submit-readiness.json");
    bind_loaded_approval_consumption_path(&mut loaded, &approval_consumption_path);
    let envelope = Phase8OperatorApprovalEnvelope {
        head_sha: "expected-head".to_string(),
        root_toml_path: "config/live.local.toml".to_string(),
        root_toml_sha256: "expected-config-hash".to_string(),
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
        operator_approval_id: "operator-approved-canary-001".to_string(),
        approval_not_before_unix_secs: 1_000,
        approval_not_after_unix_secs: 2_000,
        approval_nonce_path: approval_nonce_path.to_string_lossy().to_string(),
        approval_nonce_sha256: approval_nonce_hash,
        approval_consumption_path: approval_consumption_path.to_string_lossy().to_string(),
        canary_evidence_path: live_canary_canary_evidence_path(&loaded),
        strategy_cancel_path: live_canary_strategy_cancel_path(&loaded),
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

    let zero_window_consumption_path = temp.path().join("phase8-zero-window-consumed.json");
    let mut zero_window_loaded = loaded.clone();
    bind_loaded_approval_consumption_path(&mut zero_window_loaded, &zero_window_consumption_path);
    let mut zero_window_envelope = envelope.clone();
    zero_window_envelope.approval_not_before_unix_secs = 1_500;
    zero_window_envelope.approval_not_after_unix_secs = 1_500;
    zero_window_envelope.approval_consumption_path =
        zero_window_consumption_path.to_string_lossy().to_string();
    let zero_window_error = zero_window_envelope
        .validate_and_consume_against(
            "expected-head",
            "expected-config-hash",
            "operator-approved-canary-001",
            &zero_window_loaded,
            1_500,
        )
        .expect_err("zero-length approval window should fail closed");
    assert!(
        zero_window_error
            .to_string()
            .contains("not_after must be greater than not_before"),
        "error should mention ordered approval window: {zero_window_error}"
    );
    assert!(
        !zero_window_consumption_path.exists(),
        "zero-length approval window must not create consumption evidence"
    );

    let expired_with_drift_consumption_path =
        temp.path().join("phase8-expired-with-drift-consumed.json");
    let mut expired_with_drift_loaded = loaded.clone();
    bind_loaded_approval_consumption_path(
        &mut expired_with_drift_loaded,
        &expired_with_drift_consumption_path,
    );
    let mut expired_with_drift_envelope = envelope.clone();
    expired_with_drift_envelope.financial_envelope_sha256 =
        "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff".to_string();
    expired_with_drift_envelope.approval_consumption_path = expired_with_drift_consumption_path
        .to_string_lossy()
        .to_string();
    let expired_with_drift_error = expired_with_drift_envelope
        .validate_and_consume_against(
            "expected-head",
            "expected-config-hash",
            "operator-approved-canary-001",
            &expired_with_drift_loaded,
            2_001,
        )
        .expect_err("expired approval with drifted evidence should fail closed");
    assert!(
        expired_with_drift_error.to_string().contains("is expired"),
        "expired approval should fail before evidence drift checks: {expired_with_drift_error}"
    );
    assert!(
        !expired_with_drift_consumption_path.exists(),
        "expired approval with drifted evidence must not create consumption evidence"
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
    assert_eq!(consumption["record_kind"], APPROVAL_CONSUMPTION_RECORD_KIND);
    assert_eq!(consumption["approval_not_before_unix_secs"], 1_000);
    assert_eq!(consumption["approval_not_after_unix_secs"], 2_000);
    assert_eq!(consumption["consumed_unix_secs"], 1_500);
    assert_eq!(
        consumption["approval_envelope_sha256"],
        PHASE8_TEST_APPROVAL_ENVELOPE_SHA256
    );
    assert!(consumption.get("client_order_id_hash").is_none());
    assert!(consumption.get("venue_order_id_hash").is_none());

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

#[tokio::test]
async fn operator_approval_consumption_writer_output_is_accepted_by_live_gate() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let report_path = temp.path().join("no-submit-readiness.json");
    let approval_consumption_path = temp.path().join("phase8-approval-consumed.json");
    write_satisfied_no_submit_readiness_report(&report_path);

    let mut loaded = loaded_with_live_canary(&report_path.to_string_lossy());
    let root_toml_sha256 = Phase8OperatorApprovalEnvelope::sha256_file(&loaded.root_path)
        .expect("root TOML hash should compute");
    let operator_evidence = loaded
        .root
        .live_canary
        .as_mut()
        .and_then(|live_canary| live_canary.operator_evidence.as_mut())
        .expect("operator evidence should exist");
    operator_evidence.approval_consumption_path =
        approval_consumption_path.to_string_lossy().to_string();

    let envelope = Phase8OperatorApprovalEnvelope {
        head_sha: operator_evidence.head_sha.clone(),
        root_toml_path: loaded.root_path.to_string_lossy().to_string(),
        root_toml_sha256,
        approval_envelope_sha256: operator_evidence.approval_envelope_sha256.clone(),
        ssm_manifest_path: operator_evidence.ssm_manifest_path.clone(),
        ssm_manifest_sha256: operator_evidence.ssm_manifest_sha256.clone(),
        strategy_input_evidence_path: operator_evidence.strategy_input_evidence_path.clone(),
        strategy_input_evidence_sha256: operator_evidence.strategy_input_evidence_sha256.clone(),
        financial_envelope_path: operator_evidence.financial_envelope_path.clone(),
        financial_envelope_sha256: operator_evidence.financial_envelope_sha256.clone(),
        pre_run_state_path: operator_evidence.pre_run_state_path.clone(),
        pre_run_state_sha256: operator_evidence.pre_run_state_sha256.clone(),
        abort_plan_path: operator_evidence.abort_plan_path.clone(),
        abort_plan_sha256: operator_evidence.abort_plan_sha256.clone(),
        operator_approval_id: "operator-approved-canary-001".to_string(),
        approval_not_before_unix_secs: operator_evidence.approval_not_before_unix_seconds,
        approval_not_after_unix_secs: operator_evidence.approval_not_after_unix_seconds,
        approval_nonce_path: operator_evidence.approval_nonce_path.clone(),
        approval_nonce_sha256: operator_evidence.approval_nonce_sha256.clone(),
        approval_consumption_path: operator_evidence.approval_consumption_path.clone(),
        canary_evidence_path: operator_evidence.canary_evidence_path.clone(),
        strategy_cancel_path: operator_evidence.strategy_cancel_path.clone(),
    };

    envelope
        .consume_approval_after_live_runner_entry_validation(
            &loaded,
            current_unix_seconds_for_test() as i64,
        )
        .expect("writer-created approval consumption proof should persist");

    check_bolt_v3_live_canary_gate(&loaded)
        .await
        .expect("live gate should accept writer-created approval consumption proof");
}

#[test]
fn operator_approval_consumption_rejects_strategy_cancel_path_drift_before_spend() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let report_path = temp.path().join("no-submit-readiness.json");

    let mut loaded = loaded_with_live_canary(&report_path.to_string_lossy());
    let root_toml_sha256 = Phase8OperatorApprovalEnvelope::sha256_file(&loaded.root_path)
        .expect("root TOML hash should compute");
    let operator_evidence = loaded
        .root
        .live_canary
        .as_mut()
        .and_then(|live_canary| live_canary.operator_evidence.as_mut())
        .expect("operator evidence should exist");
    assert!(
        operator_evidence.strategy_cancel_path.is_some(),
        "fixture must configure strategy_cancel_path to prove env/TOML drift"
    );

    let base_envelope = Phase8OperatorApprovalEnvelope {
        head_sha: operator_evidence.head_sha.clone(),
        root_toml_path: loaded.root_path.to_string_lossy().to_string(),
        root_toml_sha256,
        approval_envelope_sha256: operator_evidence.approval_envelope_sha256.clone(),
        ssm_manifest_path: operator_evidence.ssm_manifest_path.clone(),
        ssm_manifest_sha256: operator_evidence.ssm_manifest_sha256.clone(),
        strategy_input_evidence_path: operator_evidence.strategy_input_evidence_path.clone(),
        strategy_input_evidence_sha256: operator_evidence.strategy_input_evidence_sha256.clone(),
        financial_envelope_path: operator_evidence.financial_envelope_path.clone(),
        financial_envelope_sha256: operator_evidence.financial_envelope_sha256.clone(),
        pre_run_state_path: operator_evidence.pre_run_state_path.clone(),
        pre_run_state_sha256: operator_evidence.pre_run_state_sha256.clone(),
        abort_plan_path: operator_evidence.abort_plan_path.clone(),
        abort_plan_sha256: operator_evidence.abort_plan_sha256.clone(),
        operator_approval_id: "operator-approved-canary-001".to_string(),
        approval_not_before_unix_secs: operator_evidence.approval_not_before_unix_seconds,
        approval_not_after_unix_secs: operator_evidence.approval_not_after_unix_seconds,
        approval_nonce_path: operator_evidence.approval_nonce_path.clone(),
        approval_nonce_sha256: operator_evidence.approval_nonce_sha256.clone(),
        approval_consumption_path: String::new(),
        canary_evidence_path: operator_evidence.canary_evidence_path.clone(),
        strategy_cancel_path: operator_evidence.strategy_cancel_path.clone(),
    };

    for (case, strategy_cancel_path) in [
        ("missing", None),
        (
            "wrong",
            Some(
                temp.path()
                    .join("wrong-strategy-cancel.json")
                    .to_string_lossy()
                    .to_string(),
            ),
        ),
    ] {
        let approval_consumption_path = temp
            .path()
            .join(format!("phase8-approval-consumed-{case}.json"));
        let mut envelope = base_envelope.clone();
        envelope.approval_consumption_path =
            approval_consumption_path.to_string_lossy().to_string();
        envelope.strategy_cancel_path = strategy_cancel_path;
        let mut loaded_for_case = loaded.clone();
        bind_loaded_approval_consumption_path(&mut loaded_for_case, &approval_consumption_path);

        let error = envelope
            .consume_approval_after_live_runner_entry_validation(
                &loaded_for_case,
                current_unix_seconds_for_test() as i64,
            )
            .expect_err("strategy_cancel_path drift must fail before spending approval");

        assert!(
            error.to_string().contains("strategy_cancel_path"),
            "error should mention strategy_cancel_path drift: {error}"
        );
        assert!(
            !approval_consumption_path.exists(),
            "strategy_cancel_path drift must not create approval consumption evidence"
        );
    }
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
        r#"{"realized_volatility":"2.5","seconds_to_market_end":300,"spot_price":"100000.0","price_to_beat_value":"100000.0","expected_edge_basis_points":"12.5","worst_case_edge_basis_points":"12.5","fee_rate_basis_points":"0","price_to_beat_source":"chainlink_data_streams.configured-reference-price","reference_quote_ts_event":1234567890,"pricing_kurtosis":"0","theta_decay_factor":"0","theta_scaled_min_edge_bps":"12.5","market_selection_timestamp_ms":1234567890,"market_selection_outcome":"current","polymarket_condition_id":"configured-condition","polymarket_market_slug":"configured-asset-updown-configuredwindow","polymarket_question_id":"configured-question","up_instrument_id":"configured-condition-UP.POLYMARKET","down_instrument_id":"configured-condition-DOWN.POLYMARKET","selected_market_observed_timestamp_ms":1234567890,"polymarket_market_start_timestamp_ms":1234567000,"polymarket_market_end_timestamp_ms":1234867000}"#,
    )
    .expect("strategy input evidence should write");
    let strategy_input_hash = Phase8OperatorApprovalEnvelope::sha256_file(&strategy_input_path)
        .expect("strategy input evidence hash should compute");
    let mut envelope = Phase8OperatorApprovalEnvelope {
        head_sha: "expected-head".to_string(),
        root_toml_path: "config/live.local.toml".to_string(),
        root_toml_sha256: "expected-config-hash".to_string(),
        approval_envelope_sha256: PHASE8_TEST_APPROVAL_ENVELOPE_SHA256.to_string(),
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
        approval_not_before_unix_secs: 1_000,
        approval_not_after_unix_secs: 2_000,
        approval_nonce_path: "phase8-approval-nonce.json".to_string(),
        approval_nonce_sha256: "expected-approval-nonce-hash".to_string(),
        approval_consumption_path: "phase8-approval-consumed.json".to_string(),
        canary_evidence_path: "phase8-canary-evidence.json".to_string(),
        strategy_cancel_path: None,
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
        r#"{"realized_volatility":"2.5","seconds_to_market_end":300,"spot_price":"100000.0","price_to_beat_value":"100000.0","expected_edge_basis_points":"12.5","worst_case_edge_basis_points":"12.5","fee_rate_basis_points":"0","price_to_beat_source":"chainlink_data_streams.configured-reference-price","reference_quote_ts_event":1234567890,"pricing_kurtosis":"0","theta_decay_factor":"0","theta_scaled_min_edge_bps":"12.5","market_selection_timestamp_ms":1234567890,"market_selection_outcome":"current","polymarket_condition_id":"configured-condition","polymarket_market_slug":"configured-asset-updown-configuredwindow","polymarket_question_id":"configured-question","up_instrument_id":"configured-condition-UP.POLYMARKET","down_instrument_id":"configured-condition-DOWN.POLYMARKET","selected_market_observed_timestamp_ms":1234567890,"polymarket_market_start_timestamp_ms":1234567000,"polymarket_market_end_timestamp_ms":1234867000}"#,
    )
    .expect("strategy input evidence should write");
    let strategy_input_hash = Phase8OperatorApprovalEnvelope::sha256_file(&strategy_input_path)
        .expect("strategy input evidence hash should compute");
    let mut envelope = Phase8OperatorApprovalEnvelope {
        head_sha: "expected-head".to_string(),
        root_toml_path: "config/live.local.toml".to_string(),
        root_toml_sha256: "expected-config-hash".to_string(),
        approval_envelope_sha256: PHASE8_TEST_APPROVAL_ENVELOPE_SHA256.to_string(),
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
        approval_not_before_unix_secs: 1_000,
        approval_not_after_unix_secs: 2_000,
        approval_nonce_path: "phase8-approval-nonce.json".to_string(),
        approval_nonce_sha256: "expected-approval-nonce-hash".to_string(),
        approval_consumption_path: "phase8-approval-consumed.json".to_string(),
        canary_evidence_path: "phase8-canary-evidence.json".to_string(),
        strategy_cancel_path: None,
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
        r#"{"realized_volatility":"2.5","seconds_to_market_end":300,"spot_price":"100000.0","price_to_beat_value":"100000.0","expected_edge_basis_points":"12.5","worst_case_edge_basis_points":"12.5","fee_rate_basis_points":"0","price_to_beat_source":"chainlink_data_streams.configured-reference-price","reference_quote_ts_event":1234567890,"pricing_kurtosis":"0","theta_decay_factor":"0","theta_scaled_min_edge_bps":"12.5","market_selection_timestamp_ms":1234567890,"market_selection_outcome":"current","polymarket_condition_id":"configured-condition","polymarket_market_slug":"configured-asset-updown-configuredwindow","polymarket_question_id":"configured-question","up_instrument_id":"configured-condition-UP.POLYMARKET","down_instrument_id":"configured-condition-DOWN.POLYMARKET","selected_market_observed_timestamp_ms":1234567890,"polymarket_market_start_timestamp_ms":1234567000,"polymarket_market_end_timestamp_ms":1234867000}"#,
    )
    .expect("strategy input evidence should write");
    let strategy_input_hash = Phase8OperatorApprovalEnvelope::sha256_file(&strategy_input_path)
        .expect("strategy input evidence hash should compute");
    let mut loaded = loaded_with_live_canary("reports/no-submit-readiness.json");
    loaded
        .root
        .live_canary
        .as_mut()
        .expect("live canary should exist")
        .max_notional_per_order = "5.00".to_string();
    let approved_oms_type = oms_type_value(loaded.strategies[0].config.oms_type);
    let financial_envelope_path = temp.path().join("phase8-financial-envelope.json");
    std::fs::write(
        &financial_envelope_path,
        serde_json::to_vec(&serde_json::json!({
            "max_live_order_count": 1,
            "max_notional_per_order": "5.00",
            "strategy_instance_id": "configured_updown_main",
            "oms_type": approved_oms_type,
            "execution_client_id": "polymarket_main",
            "configured_target_id": "configured_updown_target",
            "target_kind": "rotating_market",
            "rotating_market_family": "updown",
            "underlying_asset": "CONFIGURED_ASSET",
            "cadence_secs": 300,
            "cadence_slug_token": "configuredwindow",
            "market_selection_rule": "active_or_next",
            "retry_interval_secs": 5,
            "blocked_after_secs": 60,
            "price_to_beat_source": PHASE8_TEST_PRICE_TO_BEAT_SOURCE,
            "edge_threshold_basis_points": 100,
            "order_notional_target": "5.00",
            "maximum_position_notional": "10.00",
            "book_impact_cap_bps": 50,
            "entry_side": "buy",
            "entry_position_side": "long",
            "entry_order_type": "limit",
            "entry_time_in_force": "fok",
            "entry_is_post_only": false,
            "entry_is_reduce_only": false,
            "entry_is_quote_quantity": false,
            "exit_side": "sell",
            "exit_position_side": "long",
            "exit_order_type": "market",
            "exit_time_in_force": "ioc",
            "exit_is_post_only": false,
            "exit_is_reduce_only": false,
            "exit_is_quote_quantity": false,
            "forced_exit_side": "sell",
            "forced_exit_position_side": "long",
            "forced_exit_order_type": "market",
            "forced_exit_time_in_force": "gtc",
            "forced_exit_is_post_only": false,
            "forced_exit_is_reduce_only": true,
            "forced_exit_is_quote_quantity": false
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
    bind_loaded_approval_consumption_path(&mut loaded, &approval_consumption_path);
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
        operator_approval_id: "operator-approved-canary-001".to_string(),
        approval_not_before_unix_secs: 1_000,
        approval_not_after_unix_secs: 2_000,
        approval_nonce_path: approval_nonce_path.to_string_lossy().to_string(),
        approval_nonce_sha256: approval_nonce_hash,
        approval_consumption_path: approval_consumption_path.to_string_lossy().to_string(),
        canary_evidence_path: live_canary_canary_evidence_path(&loaded),
        strategy_cancel_path: live_canary_strategy_cancel_path(&loaded),
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

    let stale_field_financial_envelope_path = temp
        .path()
        .join("phase8-financial-envelope-stale-field.json");
    write_phase8_financial_envelope(&stale_field_financial_envelope_path, "5.00");
    let mut stale_field_financial_envelope: serde_json::Value = serde_json::from_slice(
        &std::fs::read(&stale_field_financial_envelope_path)
            .expect("stale-field financial envelope should read"),
    )
    .expect("stale-field financial envelope should parse");
    let legacy_execution_field = ["strategy", "_venue"].concat();
    stale_field_financial_envelope
        .as_object_mut()
        .expect("financial envelope should be an object")
        .insert(
            legacy_execution_field,
            serde_json::Value::String("polymarket_main".to_string()),
        );
    std::fs::write(
        &stale_field_financial_envelope_path,
        serde_json::to_vec(&stale_field_financial_envelope)
            .expect("stale-field financial envelope should serialize"),
    )
    .expect("stale-field financial envelope should write");
    let stale_field_financial_envelope_hash =
        Phase8OperatorApprovalEnvelope::sha256_file(&stale_field_financial_envelope_path)
            .expect("stale-field financial envelope hash should compute");
    let mut stale_field_envelope = envelope.clone();
    stale_field_envelope.financial_envelope_path = stale_field_financial_envelope_path
        .to_string_lossy()
        .to_string();
    stale_field_envelope.financial_envelope_sha256 = stale_field_financial_envelope_hash;
    let stale_field_error = stale_field_envelope
        .validate_and_consume_against(
            "expected-head",
            "expected-config-hash",
            "operator-approved-canary-001",
            &loaded,
            1_500,
        )
        .expect_err("stale financial envelope execution field should fail closed");
    assert!(
        stale_field_error.to_string().contains("unknown field"),
        "error should mention unknown stale financial field: {stale_field_error}"
    );
    assert!(
        !approval_consumption_path.exists(),
        "stale financial envelope field must not create consumption evidence"
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

    let mut mismatched_oms_loaded = loaded.clone();
    let approved_oms_variant = mismatched_oms_loaded.strategies[0].config.oms_type;
    mismatched_oms_loaded.strategies[0].config.oms_type = alternate_oms_type(approved_oms_variant);
    let mismatched_oms_error = envelope
        .validate_and_consume_against(
            "expected-head",
            "expected-config-hash",
            "operator-approved-canary-001",
            &mismatched_oms_loaded,
            1_500,
        )
        .expect_err("strategy OMS type mismatch against loaded TOML should fail closed");
    assert!(
        mismatched_oms_error
            .to_string()
            .contains("phase8 financial envelope `oms_type` does not match loaded TOML"),
        "error should mention mismatched OMS type: {mismatched_oms_error}"
    );
    assert!(
        !approval_consumption_path.exists(),
        "OMS type mismatch must not create consumption evidence"
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
    target.insert("retry_interval_secs".to_string(), toml::Value::Integer(6));
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
        mismatched_retry_error
            .to_string()
            .contains("phase8 financial envelope `retry_interval_secs` does not match loaded TOML"),
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
    target.insert("blocked_after_secs".to_string(), toml::Value::Integer(61));
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
        mismatched_block_error
            .to_string()
            .contains("phase8 financial envelope `blocked_after_secs` does not match loaded TOML"),
        "error should mention mismatched blocked window: {mismatched_block_error}"
    );
    assert!(
        !approval_consumption_path.exists(),
        "target blocked window mismatch must not create consumption evidence"
    );

    let mut mismatched_price_source_loaded = loaded.clone();
    let target = mismatched_price_source_loaded.strategies[0]
        .config
        .target
        .as_table_mut()
        .expect("strategy target should be a TOML table");
    let gate_mapping = target
        .get_mut("gate_subscriptions")
        .and_then(toml::Value::as_table_mut)
        .and_then(|subscriptions| subscriptions.get_mut("resolution"))
        .and_then(toml::Value::as_table_mut)
        .and_then(|resolution| resolution.get_mut("market_mappings"))
        .and_then(toml::Value::as_array_mut)
        .and_then(|mappings| mappings.first_mut())
        .and_then(toml::Value::as_table_mut)
        .expect("strategy target should include a resolution gate mapping");
    gate_mapping.insert(
        "resolution_identity".to_string(),
        toml::Value::String("operator-configured-source".to_string()),
    );
    let mismatched_price_source_error = envelope
        .validate_and_consume_against(
            "expected-head",
            "expected-config-hash",
            "operator-approved-canary-001",
            &mismatched_price_source_loaded,
            1_500,
        )
        .expect_err("price source mismatch against loaded TOML should fail closed");
    assert!(
        mismatched_price_source_error.to_string().contains(
            "phase8 financial envelope `price_to_beat_source` does not match loaded TOML"
        ),
        "error should mention mismatched price source: {mismatched_price_source_error}"
    );
    assert!(
        !approval_consumption_path.exists(),
        "price source mismatch must not create consumption evidence"
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

    let mut mismatched_forced_exit_order_loaded = loaded.clone();
    let exit_time_in_force = mismatched_forced_exit_order_loaded.strategies[0]
        .config
        .parameters
        .as_table()
        .and_then(|parameters| parameters.get("exit_order"))
        .and_then(toml::Value::as_table)
        .and_then(|exit_order| exit_order.get("time_in_force"))
        .cloned()
        .expect("strategy exit order time_in_force should exist");
    let forced_exit_order = mismatched_forced_exit_order_loaded.strategies[0]
        .config
        .parameters
        .as_table_mut()
        .and_then(|parameters| parameters.get_mut("forced_exit_order"))
        .and_then(toml::Value::as_table_mut)
        .expect("strategy forced exit order parameters should be a TOML table");
    forced_exit_order.insert("time_in_force".to_string(), exit_time_in_force);
    let forced_exit_consumption_path = temp
        .path()
        .join("phase8-approval-consumed-forced-exit-order.json");
    let mut mismatched_forced_exit_order_envelope = envelope.clone();
    mismatched_forced_exit_order_envelope.approval_consumption_path =
        forced_exit_consumption_path.to_string_lossy().to_string();
    bind_loaded_approval_consumption_path(
        &mut mismatched_forced_exit_order_loaded,
        &forced_exit_consumption_path,
    );
    let mismatched_forced_exit_order_error = mismatched_forced_exit_order_envelope
        .validate_and_consume_against(
            "expected-head",
            "expected-config-hash",
            "operator-approved-canary-001",
            &mismatched_forced_exit_order_loaded,
            1_500,
        )
        .expect_err("forced exit order drift against loaded TOML should fail closed");
    assert!(
        mismatched_forced_exit_order_error.to_string().contains(
            "phase8 financial envelope `forced_exit_time_in_force` does not match loaded TOML"
        ),
        "error should mention mismatched forced exit order field: {mismatched_forced_exit_order_error}"
    );
    assert!(
        !forced_exit_consumption_path.exists(),
        "forced exit order drift must not create consumption evidence"
    );

    for (field_key, envelope_field, value) in [
        (
            "order_type",
            "forced_exit_order_type",
            toml::Value::String("limit".to_string()),
        ),
        (
            "is_reduce_only",
            "forced_exit_is_reduce_only",
            toml::Value::Boolean(false),
        ),
    ] {
        let mut mismatched_forced_exit_required_loaded = loaded.clone();
        let forced_exit_order = mismatched_forced_exit_required_loaded.strategies[0]
            .config
            .parameters
            .as_table_mut()
            .and_then(|parameters| parameters.get_mut("forced_exit_order"))
            .and_then(toml::Value::as_table_mut)
            .expect("strategy forced exit order parameters should be a TOML table");
        forced_exit_order.insert(field_key.to_string(), value);
        let consumption_path = temp.path().join(format!(
            "phase8-approval-consumed-forced-exit-order-{field_key}.json"
        ));
        let mut mismatched_forced_exit_required_envelope = envelope.clone();
        mismatched_forced_exit_required_envelope.approval_consumption_path =
            consumption_path.to_string_lossy().to_string();
        bind_loaded_approval_consumption_path(
            &mut mismatched_forced_exit_required_loaded,
            &consumption_path,
        );
        let mismatched_forced_exit_required_error = mismatched_forced_exit_required_envelope
            .validate_and_consume_against(
                "expected-head",
                "expected-config-hash",
                "operator-approved-canary-001",
                &mismatched_forced_exit_required_loaded,
                1_500,
            )
            .expect_err("forced exit required order-shape drift should fail closed");
        assert!(
            mismatched_forced_exit_required_error
                .to_string()
                .contains(&format!(
                    "phase8 financial envelope `{envelope_field}` does not match loaded TOML"
                )),
            "error should mention mismatched forced exit required field: {mismatched_forced_exit_required_error}"
        );
        assert!(
            !consumption_path.exists(),
            "forced exit required order-shape drift must not create consumption evidence"
        );
    }

    for (order_key, field_key, envelope_field, value) in [
        (
            "entry_order",
            "expire_time_unix_nanos",
            "entry_expire_time_unix_nanos",
            toml::Value::Integer(4_102_444_800_000_000_000),
        ),
        (
            "entry_order",
            "trigger_price",
            "entry_trigger_price",
            toml::Value::Float(0.52),
        ),
        (
            "entry_order",
            "activation_price",
            "entry_activation_price",
            toml::Value::Float(0.51),
        ),
        (
            "entry_order",
            "trigger_type",
            "entry_trigger_type",
            toml::Value::String("default".to_string()),
        ),
        (
            "entry_order",
            "trigger_instrument_id",
            "entry_trigger_instrument_id",
            toml::Value::String("TRIGGER.SOURCE".to_string()),
        ),
        (
            "entry_order",
            "trailing_offset",
            "entry_trailing_offset",
            toml::Value::Float(2.5),
        ),
        (
            "entry_order",
            "trailing_offset_type",
            "entry_trailing_offset_type",
            toml::Value::String("basis_points".to_string()),
        ),
        (
            "exit_order",
            "expire_time_unix_nanos",
            "exit_expire_time_unix_nanos",
            toml::Value::Integer(4_102_444_800_000_000_000),
        ),
        (
            "exit_order",
            "trigger_price",
            "exit_trigger_price",
            toml::Value::Float(0.48),
        ),
        (
            "exit_order",
            "activation_price",
            "exit_activation_price",
            toml::Value::Float(0.47),
        ),
        (
            "exit_order",
            "trigger_type",
            "exit_trigger_type",
            toml::Value::String("default".to_string()),
        ),
        (
            "exit_order",
            "trigger_instrument_id",
            "exit_trigger_instrument_id",
            toml::Value::String("TRIGGER.SOURCE".to_string()),
        ),
        (
            "exit_order",
            "trailing_offset",
            "exit_trailing_offset",
            toml::Value::Float(3.0),
        ),
        (
            "exit_order",
            "trailing_offset_type",
            "exit_trailing_offset_type",
            toml::Value::String("ticks".to_string()),
        ),
        (
            "forced_exit_order",
            "expire_time_unix_nanos",
            "forced_exit_expire_time_unix_nanos",
            toml::Value::Integer(4_102_444_800_000_000_000),
        ),
        (
            "forced_exit_order",
            "trigger_price",
            "forced_exit_trigger_price",
            toml::Value::Float(0.48),
        ),
        (
            "forced_exit_order",
            "activation_price",
            "forced_exit_activation_price",
            toml::Value::Float(0.47),
        ),
        (
            "forced_exit_order",
            "trigger_type",
            "forced_exit_trigger_type",
            toml::Value::String("default".to_string()),
        ),
        (
            "forced_exit_order",
            "trigger_instrument_id",
            "forced_exit_trigger_instrument_id",
            toml::Value::String("TRIGGER.SOURCE".to_string()),
        ),
        (
            "forced_exit_order",
            "trailing_offset",
            "forced_exit_trailing_offset",
            toml::Value::Float(3.0),
        ),
        (
            "forced_exit_order",
            "trailing_offset_type",
            "forced_exit_trailing_offset_type",
            toml::Value::String("ticks".to_string()),
        ),
    ] {
        let mut mismatched_optional_order_field_loaded = loaded.clone();
        let order = mismatched_optional_order_field_loaded.strategies[0]
            .config
            .parameters
            .as_table_mut()
            .and_then(|parameters| parameters.get_mut(order_key))
            .and_then(toml::Value::as_table_mut)
            .expect("strategy order parameters should be a TOML table");
        order.insert(field_key.to_string(), value);
        let consumption_path = temp.path().join(format!(
            "phase8-approval-consumed-{order_key}-{field_key}.json"
        ));
        let mut mismatched_optional_order_field_envelope = envelope.clone();
        mismatched_optional_order_field_envelope.approval_consumption_path =
            consumption_path.to_string_lossy().to_string();
        bind_loaded_approval_consumption_path(
            &mut mismatched_optional_order_field_loaded,
            &consumption_path,
        );
        let mismatched_optional_order_field_error = mismatched_optional_order_field_envelope
            .validate_and_consume_against(
                "expected-head",
                "expected-config-hash",
                "operator-approved-canary-001",
                &mismatched_optional_order_field_loaded,
                1_500,
            )
            .expect_err("optional order-shape drift against loaded TOML should fail closed");
        assert!(
            mismatched_optional_order_field_error
                .to_string()
                .contains(&format!(
                    "phase8 financial envelope `{envelope_field}` does not match loaded TOML"
                )),
            "error should mention mismatched optional order-shape field: {mismatched_optional_order_field_error}"
        );
        assert!(
            !consumption_path.exists(),
            "optional order-shape drift must not create consumption evidence"
        );
    }

    for (field_key, envelope_field, value) in [
        (
            "side",
            "entry_side",
            toml::Value::String("sell".to_string()),
        ),
        (
            "position_side",
            "entry_position_side",
            toml::Value::String("short".to_string()),
        ),
    ] {
        let mut mismatched_required_order_field_loaded = loaded.clone();
        let entry_order = mismatched_required_order_field_loaded.strategies[0]
            .config
            .parameters
            .as_table_mut()
            .and_then(|parameters| parameters.get_mut("entry_order"))
            .and_then(toml::Value::as_table_mut)
            .expect("strategy entry order parameters should be a TOML table");
        entry_order.insert(field_key.to_string(), value);
        let consumption_path = temp.path().join(format!(
            "phase8-approval-consumed-entry-order-{field_key}.json"
        ));
        let mut mismatched_required_order_field_envelope = envelope.clone();
        mismatched_required_order_field_envelope.approval_consumption_path =
            consumption_path.to_string_lossy().to_string();
        bind_loaded_approval_consumption_path(
            &mut mismatched_required_order_field_loaded,
            &consumption_path,
        );
        let mismatched_required_order_field_error = mismatched_required_order_field_envelope
            .validate_and_consume_against(
                "expected-head",
                "expected-config-hash",
                "operator-approved-canary-001",
                &mismatched_required_order_field_loaded,
                1_500,
            )
            .expect_err("entry side/position drift against loaded TOML should fail closed");
        assert!(
            mismatched_required_order_field_error
                .to_string()
                .contains(&format!(
                    "phase8 financial envelope `{envelope_field}` does not match loaded TOML"
                )),
            "error should mention mismatched entry side/position field: {mismatched_required_order_field_error}"
        );
        assert!(
            !consumption_path.exists(),
            "entry side/position drift must not create consumption evidence"
        );
    }

    let uppercase_oms_financial_envelope_path = temp
        .path()
        .join("phase8-financial-envelope-uppercase-oms.json");
    let mut uppercase_oms_financial_envelope: serde_json::Value = serde_json::from_slice(
        &std::fs::read(&financial_envelope_path).expect("financial envelope should read"),
    )
    .expect("financial envelope should parse");
    uppercase_oms_financial_envelope
        .as_object_mut()
        .expect("financial envelope should be an object")
        .insert(
            "oms_type".to_string(),
            serde_json::Value::String(approved_oms_type.to_ascii_uppercase()),
        );
    std::fs::write(
        &uppercase_oms_financial_envelope_path,
        serde_json::to_vec(&uppercase_oms_financial_envelope)
            .expect("uppercase OMS financial envelope should serialize"),
    )
    .expect("uppercase OMS financial envelope should write");
    let uppercase_oms_financial_envelope_hash =
        Phase8OperatorApprovalEnvelope::sha256_file(&uppercase_oms_financial_envelope_path)
            .expect("uppercase OMS financial envelope hash should compute");
    let uppercase_oms_consumption_path = temp
        .path()
        .join("phase8-approval-consumed-uppercase-oms.json");
    let mut uppercase_oms_envelope = envelope.clone();
    uppercase_oms_envelope.financial_envelope_path = uppercase_oms_financial_envelope_path
        .to_string_lossy()
        .to_string();
    uppercase_oms_envelope.financial_envelope_sha256 = uppercase_oms_financial_envelope_hash;
    uppercase_oms_envelope.approval_consumption_path =
        uppercase_oms_consumption_path.to_string_lossy().to_string();
    let mut uppercase_oms_loaded = loaded.clone();
    bind_loaded_approval_consumption_path(
        &mut uppercase_oms_loaded,
        &uppercase_oms_consumption_path,
    );
    uppercase_oms_envelope
        .validate_and_consume_against(
            "expected-head",
            "expected-config-hash",
            "operator-approved-canary-001",
            &uppercase_oms_loaded,
            1_500,
        )
        .expect("financial envelope should canonicalize OMS through NT parsing");
    assert!(
        uppercase_oms_consumption_path.exists(),
        "NT-equivalent OMS spelling should create consumption evidence"
    );
    std::fs::remove_file(&uppercase_oms_consumption_path)
        .expect("uppercase OMS consumption evidence should remove");

    let financial_envelope_order_enum_fields = [
        "entry_side",
        "entry_position_side",
        "entry_order_type",
        "entry_time_in_force",
        "entry_trigger_type",
        "entry_trailing_offset_type",
        "exit_side",
        "exit_position_side",
        "exit_order_type",
        "exit_time_in_force",
        "exit_trigger_type",
        "exit_trailing_offset_type",
        "forced_exit_side",
        "forced_exit_position_side",
        "forced_exit_order_type",
        "forced_exit_time_in_force",
        "forced_exit_trigger_type",
        "forced_exit_trailing_offset_type",
    ];
    let uppercase_order_enums_financial_envelope_path = temp
        .path()
        .join("phase8-financial-envelope-uppercase-order-enums.json");
    let mut uppercase_order_enums_financial_envelope: serde_json::Value = serde_json::from_slice(
        &std::fs::read(&financial_envelope_path).expect("financial envelope should read"),
    )
    .expect("financial envelope should parse");
    let uppercase_order_enums = uppercase_order_enums_financial_envelope
        .as_object_mut()
        .expect("financial envelope should be an object");
    let mut changed_order_enum_fields = 0usize;
    for field in financial_envelope_order_enum_fields {
        if let Some(value) = uppercase_order_enums
            .get(field)
            .and_then(serde_json::Value::as_str)
            .map(str::to_ascii_uppercase)
        {
            if uppercase_order_enums
                .get(field)
                .and_then(serde_json::Value::as_str)
                != Some(value.as_str())
            {
                changed_order_enum_fields += 1;
            }
            uppercase_order_enums.insert(field.to_string(), serde_json::Value::String(value));
        }
    }
    assert!(
        changed_order_enum_fields > 0,
        "order enum regression should transform at least one approved envelope spelling"
    );
    let loaded_entry_side = loaded.strategies[0]
        .config
        .parameters
        .as_table()
        .and_then(|parameters| parameters.get("entry_order"))
        .and_then(toml::Value::as_table)
        .and_then(|entry_order| entry_order.get("side"))
        .and_then(toml::Value::as_str)
        .expect("loaded entry order side should exist");
    let approved_entry_side = uppercase_order_enums_financial_envelope
        .get("entry_side")
        .and_then(serde_json::Value::as_str)
        .expect("approved financial envelope entry side should exist");
    assert_ne!(
        approved_entry_side, loaded_entry_side,
        "regression must prove comparison accepts NT-equivalent non-identical order enum text"
    );
    std::fs::write(
        &uppercase_order_enums_financial_envelope_path,
        serde_json::to_vec(&uppercase_order_enums_financial_envelope)
            .expect("uppercase order-enum financial envelope should serialize"),
    )
    .expect("uppercase order-enum financial envelope should write");
    let uppercase_order_enums_financial_envelope_hash =
        Phase8OperatorApprovalEnvelope::sha256_file(&uppercase_order_enums_financial_envelope_path)
            .expect("uppercase order-enum financial envelope hash should compute");
    let uppercase_order_enums_consumption_path = temp
        .path()
        .join("phase8-approval-consumed-uppercase-order-enums.json");
    let mut uppercase_order_enums_envelope = envelope.clone();
    uppercase_order_enums_envelope.financial_envelope_path =
        uppercase_order_enums_financial_envelope_path
            .to_string_lossy()
            .to_string();
    uppercase_order_enums_envelope.financial_envelope_sha256 =
        uppercase_order_enums_financial_envelope_hash;
    uppercase_order_enums_envelope.approval_consumption_path =
        uppercase_order_enums_consumption_path
            .to_string_lossy()
            .to_string();
    let mut uppercase_order_enums_loaded = loaded.clone();
    bind_loaded_approval_consumption_path(
        &mut uppercase_order_enums_loaded,
        &uppercase_order_enums_consumption_path,
    );
    uppercase_order_enums_envelope
        .validate_and_consume_against(
            "expected-head",
            "expected-config-hash",
            "operator-approved-canary-001",
            &uppercase_order_enums_loaded,
            1_500,
        )
        .expect("financial envelope should canonicalize NT order enum spellings");
    assert!(
        uppercase_order_enums_consumption_path.exists(),
        "NT-equivalent order enum spellings should create consumption evidence"
    );
    std::fs::remove_file(&uppercase_order_enums_consumption_path)
        .expect("uppercase order-enum consumption evidence should remove");

    let mut uppercase_order_enums_loaded = loaded.clone();
    let uppercase_order_parameters = uppercase_order_enums_loaded.strategies[0]
        .config
        .parameters
        .as_table_mut()
        .expect("strategy parameters should be a TOML table");
    for (order_key, order_fields) in [
        (
            "entry_order",
            [
                "side",
                "position_side",
                "order_type",
                "time_in_force",
                "trigger_type",
                "trailing_offset_type",
            ],
        ),
        (
            "exit_order",
            [
                "side",
                "position_side",
                "order_type",
                "time_in_force",
                "trigger_type",
                "trailing_offset_type",
            ],
        ),
        (
            "forced_exit_order",
            [
                "side",
                "position_side",
                "order_type",
                "time_in_force",
                "trigger_type",
                "trailing_offset_type",
            ],
        ),
    ] {
        let order = uppercase_order_parameters
            .get_mut(order_key)
            .and_then(toml::Value::as_table_mut)
            .expect("strategy order parameters should be a TOML table");
        for field in order_fields {
            if let Some(value) = order
                .get(field)
                .and_then(toml::Value::as_str)
                .map(str::to_ascii_uppercase)
            {
                order.insert(field.to_string(), toml::Value::String(value));
            }
        }
    }
    let uppercase_loaded_order_enums_consumption_path = temp
        .path()
        .join("phase8-approval-consumed-uppercase-loaded-order-enums.json");
    let mut uppercase_loaded_order_enums_envelope = envelope.clone();
    uppercase_loaded_order_enums_envelope.approval_consumption_path =
        uppercase_loaded_order_enums_consumption_path
            .to_string_lossy()
            .to_string();
    bind_loaded_approval_consumption_path(
        &mut uppercase_order_enums_loaded,
        &uppercase_loaded_order_enums_consumption_path,
    );
    uppercase_loaded_order_enums_envelope
        .validate_and_consume_against(
            "expected-head",
            "expected-config-hash",
            "operator-approved-canary-001",
            &uppercase_order_enums_loaded,
            1_500,
        )
        .expect("loaded TOML order enum spellings should canonicalize before comparison");
    assert!(
        uppercase_loaded_order_enums_consumption_path.exists(),
        "NT-equivalent loaded TOML order enum spellings should create consumption evidence"
    );
    std::fs::remove_file(&uppercase_loaded_order_enums_consumption_path)
        .expect("uppercase loaded order-enum consumption evidence should remove");

    let invalid_order_enum_financial_envelope_path = temp
        .path()
        .join("phase8-financial-envelope-invalid-order-enum.json");
    let mut invalid_order_enum_financial_envelope: serde_json::Value = serde_json::from_slice(
        &std::fs::read(&financial_envelope_path).expect("financial envelope should read"),
    )
    .expect("financial envelope should parse");
    let invalid_order_enum_field = financial_envelope_order_enum_fields
        .iter()
        .find_map(|field| {
            invalid_order_enum_financial_envelope
                .get(*field)
                .and_then(serde_json::Value::as_str)
                .map(|value| (*field, format!("{value}_invalid")))
        })
        .expect("financial envelope should contain at least one order enum field");
    invalid_order_enum_financial_envelope
        .as_object_mut()
        .expect("financial envelope should be an object")
        .insert(
            invalid_order_enum_field.0.to_string(),
            serde_json::Value::String(invalid_order_enum_field.1),
        );
    std::fs::write(
        &invalid_order_enum_financial_envelope_path,
        serde_json::to_vec(&invalid_order_enum_financial_envelope)
            .expect("invalid order-enum financial envelope should serialize"),
    )
    .expect("invalid order-enum financial envelope should write");
    let invalid_order_enum_financial_envelope_hash =
        Phase8OperatorApprovalEnvelope::sha256_file(&invalid_order_enum_financial_envelope_path)
            .expect("invalid order-enum financial envelope hash should compute");
    let invalid_order_enum_consumption_path = temp
        .path()
        .join("phase8-approval-consumed-invalid-order-enum.json");
    let mut invalid_order_enum_envelope = envelope.clone();
    invalid_order_enum_envelope.financial_envelope_path =
        invalid_order_enum_financial_envelope_path
            .to_string_lossy()
            .to_string();
    invalid_order_enum_envelope.financial_envelope_sha256 =
        invalid_order_enum_financial_envelope_hash;
    invalid_order_enum_envelope.approval_consumption_path = invalid_order_enum_consumption_path
        .to_string_lossy()
        .to_string();
    let mut invalid_order_enum_loaded = loaded.clone();
    bind_loaded_approval_consumption_path(
        &mut invalid_order_enum_loaded,
        &invalid_order_enum_consumption_path,
    );
    let invalid_order_enum_error = invalid_order_enum_envelope
        .validate_and_consume_against(
            "expected-head",
            "expected-config-hash",
            "operator-approved-canary-001",
            &invalid_order_enum_loaded,
            1_500,
        )
        .expect_err("unparseable financial envelope order enum should fail closed");
    assert!(
        invalid_order_enum_error.to_string().contains(&format!(
            "phase8 financial envelope `{}` must be a NautilusTrader",
            invalid_order_enum_field.0
        )),
        "error should mention invalid order enum parsing: {invalid_order_enum_error}"
    );
    assert!(
        !invalid_order_enum_consumption_path.exists(),
        "invalid order enum must not create consumption evidence"
    );

    let invalid_oms_financial_envelope_path = temp
        .path()
        .join("phase8-financial-envelope-invalid-oms.json");
    let mut invalid_oms_financial_envelope: serde_json::Value = serde_json::from_slice(
        &std::fs::read(&financial_envelope_path).expect("financial envelope should read"),
    )
    .expect("financial envelope should parse");
    invalid_oms_financial_envelope
        .as_object_mut()
        .expect("financial envelope should be an object")
        .insert(
            "oms_type".to_string(),
            serde_json::Value::String(format!("{approved_oms_type}_invalid")),
        );
    std::fs::write(
        &invalid_oms_financial_envelope_path,
        serde_json::to_vec(&invalid_oms_financial_envelope)
            .expect("invalid OMS financial envelope should serialize"),
    )
    .expect("invalid OMS financial envelope should write");
    let invalid_oms_financial_envelope_hash =
        Phase8OperatorApprovalEnvelope::sha256_file(&invalid_oms_financial_envelope_path)
            .expect("invalid OMS financial envelope hash should compute");
    let invalid_oms_consumption_path = temp
        .path()
        .join("phase8-approval-consumed-invalid-oms.json");
    let mut invalid_oms_envelope = envelope.clone();
    invalid_oms_envelope.financial_envelope_path = invalid_oms_financial_envelope_path
        .to_string_lossy()
        .to_string();
    invalid_oms_envelope.financial_envelope_sha256 = invalid_oms_financial_envelope_hash;
    invalid_oms_envelope.approval_consumption_path =
        invalid_oms_consumption_path.to_string_lossy().to_string();
    let mut invalid_oms_loaded = loaded.clone();
    bind_loaded_approval_consumption_path(&mut invalid_oms_loaded, &invalid_oms_consumption_path);
    let invalid_oms_error = invalid_oms_envelope
        .validate_and_consume_against(
            "expected-head",
            "expected-config-hash",
            "operator-approved-canary-001",
            &invalid_oms_loaded,
            1_500,
        )
        .expect_err("unparseable financial envelope OMS should fail closed");
    assert!(
        invalid_oms_error
            .to_string()
            .contains("phase8 financial envelope `oms_type` must be a NautilusTrader OmsType"),
        "error should mention invalid OMS parsing: {invalid_oms_error}"
    );
    assert!(
        !invalid_oms_consumption_path.exists(),
        "invalid OMS must not create consumption evidence"
    );

    let mut multi_strategy_loaded = loaded.clone();
    let mut secondary_strategy = multi_strategy_loaded.strategies[0].clone();
    secondary_strategy.config.strategy_instance_id = "bitcoin_updown_secondary".to_string();
    multi_strategy_loaded.strategies.push(secondary_strategy);
    let multi_strategy_consumption_path = temp
        .path()
        .join("phase8-approval-consumed-multi-strategy.json");
    let mut multi_strategy_envelope = envelope.clone();
    multi_strategy_envelope.approval_consumption_path = multi_strategy_consumption_path
        .to_string_lossy()
        .to_string();
    bind_loaded_approval_consumption_path(
        &mut multi_strategy_loaded,
        &multi_strategy_consumption_path,
    );
    multi_strategy_envelope
        .validate_and_consume_against(
            "expected-head",
            "expected-config-hash",
            "operator-approved-canary-001",
            &multi_strategy_loaded,
            1_500,
        )
        .expect(
            "financial envelope should bind the approved strategy by id in multi-strategy config",
        );
    assert!(
        multi_strategy_consumption_path.exists(),
        "multi-strategy validation should create consumption evidence for the approved strategy"
    );
    std::fs::remove_file(&multi_strategy_consumption_path)
        .expect("multi-strategy consumption evidence should remove");

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
        r#"{"realized_volatility":"2.5","seconds_to_market_end":300,"spot_price":"100000.0","price_to_beat_value":"100000.0","expected_edge_basis_points":"12.5","worst_case_edge_basis_points":"12.5","fee_rate_basis_points":"0","price_to_beat_source":"chainlink_data_streams.configured-reference-price","reference_quote_ts_event":1234567890,"pricing_kurtosis":"0","theta_decay_factor":"0","theta_scaled_min_edge_bps":"12.5","market_selection_timestamp_ms":1234567890,"market_selection_outcome":"current","polymarket_condition_id":"configured-condition","polymarket_market_slug":"configured-asset-updown-configuredwindow","polymarket_question_id":"configured-question","up_instrument_id":"configured-condition-UP.POLYMARKET","down_instrument_id":"configured-condition-DOWN.POLYMARKET","selected_market_observed_timestamp_ms":1234567890,"polymarket_market_start_timestamp_ms":1234567000,"polymarket_market_end_timestamp_ms":1234867000}"#,
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
    let mut loaded = loaded_with_live_canary("reports/no-submit-readiness.json");
    bind_loaded_approval_consumption_path(&mut loaded, &approval_consumption_path);
    let envelope = Phase8OperatorApprovalEnvelope {
        head_sha: "expected-head".to_string(),
        root_toml_path: "config/live.local.toml".to_string(),
        root_toml_sha256: "expected-config-hash".to_string(),
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
        operator_approval_id: "operator-approved-canary-001".to_string(),
        approval_not_before_unix_secs: 1_000,
        approval_not_after_unix_secs: 2_000,
        approval_nonce_path: approval_nonce_path.to_string_lossy().to_string(),
        approval_nonce_sha256: approval_nonce_hash,
        approval_consumption_path: approval_consumption_path.to_string_lossy().to_string(),
        canary_evidence_path: live_canary_canary_evidence_path(&loaded),
        strategy_cancel_path: live_canary_strategy_cancel_path(&loaded),
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

    write_phase8_pre_run_state_with_clob_fee_behavior(&pre_run_state_path, false, false);
    let blocked_clob_fee_hash = Phase8OperatorApprovalEnvelope::sha256_file(&pre_run_state_path)
        .expect("pre-run state hash should compute");
    let mut blocked_clob_fee_envelope = envelope.clone();
    blocked_clob_fee_envelope.pre_run_state_sha256 = blocked_clob_fee_hash;
    let blocked_clob_fee_error = blocked_clob_fee_envelope
        .validate_and_consume_against(
            "expected-head",
            "expected-config-hash",
            "operator-approved-canary-001",
            &loaded,
            1_500,
        )
        .expect_err("unverified CLOB V2 fee behavior must fail closed");
    assert!(
        blocked_clob_fee_error
            .to_string()
            .contains("clob_v2_fee_behavior_verified"),
        "error should mention blocked CLOB V2 fee proof: {blocked_clob_fee_error}"
    );
    assert!(
        !approval_consumption_path.exists(),
        "unverified CLOB V2 fee behavior must not create consumption evidence"
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
fn operator_approval_envelope_rejects_pre_run_state_without_artifact_hashes() {
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
        r#"{"realized_volatility":"2.5","seconds_to_market_end":300,"spot_price":"100000.0","price_to_beat_value":"100000.0","expected_edge_basis_points":"12.5","worst_case_edge_basis_points":"12.5","fee_rate_basis_points":"0","price_to_beat_source":"chainlink_data_streams.configured-reference-price","reference_quote_ts_event":1234567890,"pricing_kurtosis":"0","theta_decay_factor":"0","theta_scaled_min_edge_bps":"12.5","market_selection_timestamp_ms":1234567890,"market_selection_outcome":"current","polymarket_condition_id":"configured-condition","polymarket_market_slug":"configured-asset-updown-configuredwindow","polymarket_question_id":"configured-question","up_instrument_id":"configured-condition-UP.POLYMARKET","down_instrument_id":"configured-condition-DOWN.POLYMARKET","selected_market_observed_timestamp_ms":1234567890,"polymarket_market_start_timestamp_ms":1234567000,"polymarket_market_end_timestamp_ms":1234867000}"#,
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
    let pre_run_json = serde_json::json!({
        "execution_client_id": "polymarket_main",
        "configured_target_id": "configured_updown_target",
        "host_clock_skew_within_bound": true,
        "conflicting_open_orders_absent": true,
        "preexisting_position_absent": true,
        "market_state_approved": true,
        "market_window_approved": true,
        "funding_margin_covers_max_notional_plus_fees": true,
        "single_runner_lock_acquired": true,
        "egress_identity_approved": true,
        "clob_v2_adapter_signing_verified": true,
        "clob_v2_collateral_accounting_verified": true,
        "clob_v2_fee_behavior_verified": true,
        "release_manifest_clob_signing_version": "clob_v2",
        "release_manifest_nt_revision_matches_compiled_pin": true
    });
    std::fs::write(
        &pre_run_state_path,
        serde_json::to_vec(&pre_run_json).expect("pre-run state should serialize"),
    )
    .expect("pre-run state should write");
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
    bind_loaded_approval_consumption_path(&mut loaded, &approval_consumption_path);
    let envelope = Phase8OperatorApprovalEnvelope {
        head_sha: "expected-head".to_string(),
        root_toml_path: "config/live.local.toml".to_string(),
        root_toml_sha256: "expected-config-hash".to_string(),
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
        operator_approval_id: "operator-approved-canary-001".to_string(),
        approval_not_before_unix_secs: 1_000,
        approval_not_after_unix_secs: 2_000,
        approval_nonce_path: approval_nonce_path.to_string_lossy().to_string(),
        approval_nonce_sha256: approval_nonce_hash,
        approval_consumption_path: approval_consumption_path.to_string_lossy().to_string(),
        canary_evidence_path: live_canary_canary_evidence_path(&loaded),
        strategy_cancel_path: live_canary_strategy_cancel_path(&loaded),
    };

    let error = envelope
        .validate_and_consume_against(
            "expected-head",
            "expected-config-hash",
            "operator-approved-canary-001",
            &loaded,
            1_500,
        )
        .expect_err("pre-run state without artifact hashes should fail closed");
    assert!(
        error.to_string().contains("host_clock_skew_evidence_hash"),
        "error should mention missing pre-run artifact hash: {error}"
    );
    assert!(
        !approval_consumption_path.exists(),
        "pre-run state without artifact hashes must not consume approval"
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
        r#"{"realized_volatility":"2.5","seconds_to_market_end":300,"spot_price":"100000.0","price_to_beat_value":"100000.0","expected_edge_basis_points":"12.5","worst_case_edge_basis_points":"12.5","fee_rate_basis_points":"0","price_to_beat_source":"chainlink_data_streams.configured-reference-price","reference_quote_ts_event":1234567890,"pricing_kurtosis":"0","theta_decay_factor":"0","theta_scaled_min_edge_bps":"12.5","market_selection_timestamp_ms":1234567890,"market_selection_outcome":"current","polymarket_condition_id":"configured-condition","polymarket_market_slug":"configured-asset-updown-configuredwindow","polymarket_question_id":"configured-question","up_instrument_id":"configured-condition-UP.POLYMARKET","down_instrument_id":"configured-condition-DOWN.POLYMARKET","selected_market_observed_timestamp_ms":1234567890,"polymarket_market_start_timestamp_ms":1234567000,"polymarket_market_end_timestamp_ms":1234867000}"#,
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
    let mut loaded = loaded_with_live_canary("reports/no-submit-readiness.json");
    bind_loaded_approval_consumption_path(&mut loaded, &approval_consumption_path);
    let envelope = Phase8OperatorApprovalEnvelope {
        head_sha: "expected-head".to_string(),
        root_toml_path: "config/live.local.toml".to_string(),
        root_toml_sha256: "expected-config-hash".to_string(),
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
        operator_approval_id: "operator-approved-canary-001".to_string(),
        approval_not_before_unix_secs: 1_000,
        approval_not_after_unix_secs: 2_000,
        approval_nonce_path: approval_nonce_path.to_string_lossy().to_string(),
        approval_nonce_sha256: approval_nonce_hash,
        approval_consumption_path: approval_consumption_path.to_string_lossy().to_string(),
        canary_evidence_path: live_canary_canary_evidence_path(&loaded),
        strategy_cancel_path: live_canary_strategy_cancel_path(&loaded),
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
        readiness_report_max_age_seconds: 60,
        reference_quote_max_age_seconds: 10,
        reference_quote_wait_timeout_seconds: 10,
        reference_quote_probe_actor_id: "no-submit-reference-quote-probe".to_string(),
        reference_quote_probe_log_events: true,
        reference_quote_probe_log_commands: true,
        egress_identity_observed_path: None,
        egress_identity_observed_max_bytes: None,
        approved_egress_identity_sha256: None,
        proof_policy: None,
        operator_evidence: Some(support::valid_live_canary_operator_evidence()),
        max_live_order_count: 1,
        max_notional_per_order: "0.25".to_string(),
    });
    loaded
}

fn bind_loaded_approval_consumption_path(
    loaded: &mut LoadedBoltV3Config,
    approval_consumption_path: &Path,
) {
    loaded
        .root
        .live_canary
        .as_mut()
        .and_then(|live_canary| live_canary.operator_evidence.as_mut())
        .expect("live canary operator evidence should exist")
        .approval_consumption_path = approval_consumption_path.to_string_lossy().to_string();
}

fn live_canary_strategy_cancel_path(loaded: &LoadedBoltV3Config) -> Option<String> {
    loaded
        .root
        .live_canary
        .as_ref()
        .and_then(|live_canary| live_canary.operator_evidence.as_ref())
        .and_then(|operator_evidence| operator_evidence.strategy_cancel_path.clone())
}

fn live_canary_canary_evidence_path(loaded: &LoadedBoltV3Config) -> String {
    loaded
        .root
        .live_canary
        .as_ref()
        .and_then(|live_canary| live_canary.operator_evidence.as_ref())
        .map(|operator_evidence| operator_evidence.canary_evidence_path.clone())
        .expect("live canary operator evidence should configure canary_evidence_path")
}

fn alternate_oms_type(approved: OmsType) -> OmsType {
    [OmsType::Netting, OmsType::Hedging]
        .into_iter()
        .find(|candidate| *candidate != approved)
        .expect("NT OMS type alternatives should include a non-approved variant")
}

#[test]
fn phase8_oms_alternate_helper_covers_nt_oms_variants() {
    for approved in [OmsType::Netting, OmsType::Hedging, OmsType::Unspecified] {
        assert_ne!(alternate_oms_type(approved), approved);
    }
}

fn oms_type_value(oms_type: OmsType) -> String {
    oms_type.to_string().to_ascii_lowercase()
}

fn write_satisfied_no_submit_readiness_report(path: &std::path::Path) {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let json = serde_json::json!({
        SCHEMA_VERSION_KEY: NO_SUBMIT_READINESS_SCHEMA_VERSION,
        APPROVAL_ID_HASH_KEY: sha256_hex("operator-approved-canary-001".as_bytes()),
        EXECUTABLE_IDENTITY_KEY: current_executable_identity(),
        CONFIG_BUNDLE_CHECKSUM_KEY: loaded.config_bundle_checksum,
        GENERATED_AT_UNIX_SECONDS_KEY: current_unix_seconds_for_test(),
        STAGES_KEY: [
            {STAGE_KEY: OPERATOR_APPROVAL_STAGE, STATUS_KEY: STATUS_SATISFIED},
            {STAGE_KEY: SECRET_RESOLUTION_STAGE, STATUS_KEY: STATUS_SATISFIED},
            {STAGE_KEY: LIVE_NODE_BUILD_STAGE, STATUS_KEY: STATUS_SATISFIED},
            {STAGE_KEY: CONTROLLED_CONNECT_STAGE, STATUS_KEY: STATUS_SATISFIED},
            {STAGE_KEY: REFERENCE_READINESS_STAGE, STATUS_KEY: STATUS_SATISFIED},
            {STAGE_KEY: CONTROLLED_DISCONNECT_STAGE, STATUS_KEY: STATUS_SATISFIED},
            {STAGE_KEY: REPORT_WRITE_STAGE, STATUS_KEY: STATUS_SATISFIED}
        ]
    });
    std::fs::create_dir_all(path.parent().expect("report parent should exist"))
        .expect("report parent should create");
    std::fs::write(
        path,
        serde_json::to_vec(&json).expect("report should serialize"),
    )
    .expect("report should write");
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

fn sha256_text_for_test(value: &str) -> String {
    sha256_hex(value.as_bytes())
}

fn write_phase8_financial_envelope(path: &std::path::Path, max_notional_per_order: &str) {
    let loaded = loaded_with_live_canary("reports/no-submit-readiness.json");
    let approved_oms_type = oms_type_value(loaded.strategies[0].config.oms_type);
    let json = serde_json::json!({
        "max_live_order_count": 1,
        "max_notional_per_order": max_notional_per_order,
        "strategy_instance_id": "configured_updown_main",
        "oms_type": approved_oms_type,
        "execution_client_id": "polymarket_main",
        "configured_target_id": "configured_updown_target",
        "target_kind": "rotating_market",
        "rotating_market_family": "updown",
        "underlying_asset": "CONFIGURED_ASSET",
        "cadence_secs": 300,
        "cadence_slug_token": "configuredwindow",
        "market_selection_rule": "active_or_next",
        "retry_interval_secs": 5,
        "blocked_after_secs": 60,
        "price_to_beat_source": PHASE8_TEST_PRICE_TO_BEAT_SOURCE,
        "edge_threshold_basis_points": 100,
        "order_notional_target": "5.00",
        "maximum_position_notional": "10.00",
        "book_impact_cap_bps": 50,
        "entry_side": "buy",
        "entry_position_side": "long",
        "entry_order_type": "limit",
        "entry_time_in_force": "fok",
        "entry_is_post_only": false,
        "entry_is_reduce_only": false,
        "entry_is_quote_quantity": false,
        "exit_side": "sell",
        "exit_position_side": "long",
        "exit_order_type": "market",
        "exit_time_in_force": "ioc",
        "exit_is_post_only": false,
        "exit_is_reduce_only": false,
        "exit_is_quote_quantity": false,
        "forced_exit_side": "sell",
        "forced_exit_position_side": "long",
        "forced_exit_order_type": "market",
        "forced_exit_time_in_force": "gtc",
        "forced_exit_is_post_only": false,
        "forced_exit_is_reduce_only": true,
        "forced_exit_is_quote_quantity": false
    });
    std::fs::write(
        path,
        serde_json::to_vec(&json).expect("financial envelope should serialize"),
    )
    .expect("financial envelope should write");
}

fn write_phase8_pre_run_state(path: &std::path::Path, has_preexisting_position: bool) {
    write_phase8_pre_run_state_with_clob_fee_behavior(path, has_preexisting_position, true);
}

fn write_phase8_pre_run_state_with_clob_fee_behavior(
    path: &std::path::Path,
    has_preexisting_position: bool,
    clob_v2_fee_behavior_verified: bool,
) {
    let evidence_hash = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let json = serde_json::json!({
        "execution_client_id": "polymarket_main",
        "configured_target_id": "configured_updown_target",
        "host_clock_skew_within_bound": true,
        "host_clock_skew_evidence_hash": evidence_hash,
        "conflicting_open_orders_absent": true,
        "preexisting_position_absent": !has_preexisting_position,
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
        "clob_v2_fee_behavior_verified": clob_v2_fee_behavior_verified,
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

fn write_phase8_abort_plan(path: &std::path::Path, panic_policy_missing: bool) {
    let json = serde_json::json!({
        "execution_client_id": "polymarket_main",
        "configured_target_id": "configured_updown_target",
        "source_collector_derived": true,
        "strategy_source_sha256": sha256_text_for_test(include_str!("../src/strategies/binary_oracle_edge_taker.rs")),
        "submit_admission_source_sha256": sha256_text_for_test(include_str!("../src/bolt_v3_submit_admission.rs")),
        "cancel_if_open_defined": true,
        "cancel_if_open_evidence_hash": sha256_text_for_test("cancel-if-open-proof"),
        "nt_accepted_venue_pending_abort_defined": true,
        "nt_accepted_venue_pending_abort_evidence_hash": sha256_text_for_test("nt-accepted-venue-pending-proof"),
        "partial_fill_abort_defined": true,
        "partial_fill_abort_evidence_hash": sha256_text_for_test("partial-fill-proof"),
        "network_partition_during_submit_abort_defined": true,
        "network_partition_during_submit_abort_evidence_hash": sha256_text_for_test("network-partition-proof"),
        "panic_gate_trip_abort_defined": !panic_policy_missing,
        "panic_gate_trip_abort_evidence_hash": sha256_text_for_test("panic-gate-service-policy-proof")
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

fn valid_evidence_ref(path_prefix: &str, record_prefix: &str) -> Phase8EvidenceRef {
    Phase8EvidenceRef {
        path_hash: path_prefix.repeat(16),
        record_hash: record_prefix.repeat(16),
    }
}

fn valid_live_order_ref() -> Phase8LiveOrderRef {
    Phase8LiveOrderRef {
        strategy_instance_id_hash:
            "1212121212121212121212121212121212121212121212121212121212121212".to_string(),
        client_order_id_hash: "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"
            .to_string(),
        venue_order_id_hash: "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"
            .to_string(),
    }
}

fn valid_live_canary_result_refs() -> Phase8LiveCanaryResultRefs {
    Phase8LiveCanaryResultRefs {
        nt_submit_event_ref: valid_evidence_ref("1111", "2222"),
        venue_order_state_ref: valid_evidence_ref("3333", "4444"),
        strategy_cancel_ref: Some(valid_evidence_ref("5555", "6666")),
        restart_reconciliation_ref: valid_evidence_ref("7777", "8888"),
        post_run_hygiene_ref: valid_evidence_ref("9999", "aaaa"),
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
        approved_strategy_instance_id_hash:
            "1212121212121212121212121212121212121212121212121212121212121212".to_string(),
        approval_id: "operator-approved-canary-001".to_string(),
        max_live_order_count: 1,
        max_notional_per_order: Decimal::new(25, 2),
        runtime_capture_ref: runtime_capture_ref(),
    }
}
