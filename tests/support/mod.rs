#![allow(dead_code)]

pub(crate) mod stub_runtime_strategy;

use std::{
    any::Any,
    cell::RefCell,
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    rc::Rc,
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use async_trait::async_trait;
use bolt_v2::bolt_v3_config::LiveCanaryOperatorEvidenceBlock;
use bolt_v2::bolt_v3_no_submit_readiness_schema::{
    APPROVAL_CONSUMPTION_RECORD_KIND, APPROVAL_CONSUMPTION_SCHEMA_VERSION,
};
use nautilus_common::enums::Environment;
use nautilus_common::factories::{ClientConfig, DataClientFactory, ExecutionClientFactory};
use nautilus_common::{
    cache::CacheView,
    clients::{DataClient, ExecutionClient},
    clock::Clock,
    messages::data::{SubscribeInstrument, SubscribeQuotes, SubscribeTrades},
    messages::execution::SubmitOrder,
};
use nautilus_live::node::LiveNode;
use nautilus_model::{
    accounts::AccountAny,
    enums::OmsType,
    identifiers::{AccountId, ClientId, ClientOrderId, InstrumentId, StrategyId, TraderId, Venue},
    types::{AccountBalance, MarginBalance},
};
use sha2::{Digest, Sha256};

const TEST_DELAY_POST_STOP_SECS: u64 = 0;
const TEST_TRADER_ID: &str = "TESTER-001";

#[track_caller]
pub fn fast_test_live_node() -> LiveNode {
    LiveNode::builder(TraderId::from(TEST_TRADER_ID), Environment::Live)
        .expect("LiveNode builder should initialize with test defaults")
        .with_delay_post_stop_secs(TEST_DELAY_POST_STOP_SECS)
        .build()
        .expect("LiveNode should build with test defaults")
}

static TEMP_DIR_COUNTER: AtomicU64 = AtomicU64::new(0);
static MOCK_DATA_SUBSCRIPTIONS: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
static MOCK_EXEC_SUBMISSIONS: OnceLock<Mutex<Vec<RecordedSubmitOrder>>> = OnceLock::new();

#[derive(Debug, Default)]
pub struct RecordingDecisionEvidenceWriter {
    records: Mutex<Vec<bolt_v2::bolt_v3_decision_evidence::BoltV3OrderIntentEvidence>>,
    admission_decisions:
        Mutex<Vec<bolt_v2::bolt_v3_decision_evidence::BoltV3AdmissionDecisionEvidence>>,
}

impl RecordingDecisionEvidenceWriter {
    pub fn records(&self) -> Vec<bolt_v2::bolt_v3_decision_evidence::BoltV3OrderIntentEvidence> {
        self.records
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub fn admission_decisions(
        &self,
    ) -> Vec<bolt_v2::bolt_v3_decision_evidence::BoltV3AdmissionDecisionEvidence> {
        self.admission_decisions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

impl bolt_v2::bolt_v3_decision_evidence::BoltV3DecisionEvidenceWriter
    for RecordingDecisionEvidenceWriter
{
    fn record_strategy_input_snapshot(
        &self,
        _snapshot: &bolt_v2::bolt_v3_decision_evidence::BoltV3StrategyInputEvidenceSnapshot,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    fn record_order_intent(
        &self,
        intent: &bolt_v2::bolt_v3_decision_evidence::BoltV3OrderIntentEvidence,
    ) -> anyhow::Result<()> {
        self.records
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(intent.clone());
        Ok(())
    }

    fn record_admission_decision(
        &self,
        decision: &bolt_v2::bolt_v3_decision_evidence::BoltV3AdmissionDecisionEvidence,
    ) -> anyhow::Result<()> {
        self.admission_decisions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(decision.clone());
        Ok(())
    }
}

fn mock_data_subscriptions() -> &'static Mutex<Vec<String>> {
    MOCK_DATA_SUBSCRIPTIONS.get_or_init(|| Mutex::new(Vec::new()))
}

fn mock_exec_submissions() -> &'static Mutex<Vec<RecordedSubmitOrder>> {
    MOCK_EXEC_SUBMISSIONS.get_or_init(|| Mutex::new(Vec::new()))
}

pub fn clear_mock_data_subscriptions() {
    mock_data_subscriptions().lock().unwrap().clear();
}

pub fn recorded_mock_data_subscriptions() -> Vec<String> {
    mock_data_subscriptions().lock().unwrap().clone()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedSubmitOrder {
    pub client_id: Option<ClientId>,
    pub strategy_id: StrategyId,
    pub instrument_id: InstrumentId,
    pub client_order_id: ClientOrderId,
}

pub fn clear_mock_exec_submissions() {
    mock_exec_submissions().lock().unwrap().clear();
}

pub fn recorded_mock_exec_submissions() -> Vec<RecordedSubmitOrder> {
    mock_exec_submissions().lock().unwrap().clone()
}

pub struct TempCaseDir {
    path: PathBuf,
}

impl TempCaseDir {
    pub fn new(label: &str) -> Self {
        let timestamp_nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should move forward")
            .as_nanos();
        let counter = TEMP_DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
        let dirname = format!("bolt-v2-{label}-{timestamp_nanos}-{counter}");
        let path = std::env::temp_dir().join(dirname);
        fs::create_dir_all(&path).expect("temp case dir should be created");
        Self { path }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn persist(self) -> PathBuf {
        let path = self.path.clone();
        std::mem::forget(self);
        path
    }
}

pub fn repo_path(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(relative)
}

pub fn repo_text(relative: &str) -> String {
    fs::read_to_string(repo_path(relative))
        .unwrap_or_else(|error| panic!("repo text `{relative}` should read: {error}"))
}

pub fn valid_live_canary_operator_evidence() -> LiveCanaryOperatorEvidenceBlock {
    let case_dir = live_canary_operator_evidence_case_dir();
    let now_u64 = current_unix_seconds();
    let now = i64::try_from(now_u64).expect("current unix seconds should fit in i64");
    let one_second_ms: u64 = std::time::Duration::from_secs(1)
        .as_millis()
        .try_into()
        .expect("one second should fit in u64 milliseconds");
    let now_ms = now_u64.saturating_mul(one_second_ms);
    let approval_not_before_unix_seconds = now - 60;
    let approval_not_after_unix_seconds = now + 3600;
    let ssm_manifest_path = write_dummy_json(
        &case_dir,
        "ssm-manifest.json",
        serde_json::json!({"record_kind": "test_ssm_manifest"}),
    );
    let strategy_input_evidence_path = write_dummy_json(
        &case_dir,
        "strategy-input.json",
        serde_json::json!({
            "record_kind": "test_strategy_input",
            "strategy_instance_id": "configured_updown_main",
            "gate_session_hash": "a".repeat(64),
            "selected_market_key": "b".repeat(64),
            "gate_evidence": {
                "decision_reference": {
                    "satisfaction_kind": "evidence",
                    "selected_market_key": "b".repeat(64),
                    "provider_id": "resolution_oracle_primary",
                    "provider_kind": "chainlink_data_streams",
                    "value_kind": "price",
                    "normalized_value_sha256": "c".repeat(64),
                    "provider_provenance_sha256": "d".repeat(64),
                    "artifact_sha256s": ["e".repeat(64)]
                },
                "resolution": {
                    "satisfaction_kind": "no_resolution",
                    "selected_market_key": "b".repeat(64),
                    "resolution_identity": "configured-reference-price",
                    "artifact_sha256s": []
                }
            },
            "realized_volatility": "1.5",
            "spot_price": "3101",
            "price_to_beat_value": "3100",
            "reference_quote_ts_event": now_ms,
            "polymarket_condition_id": "configured-condition",
            "polymarket_market_slug": "configured-market",
            "polymarket_question_id": "configured-question",
            "up_instrument_id": "configured-condition-UP.POLYMARKET",
            "down_instrument_id": "configured-condition-DOWN.POLYMARKET",
            "polymarket_market_start_timestamp_ms": now_ms,
            "polymarket_market_end_timestamp_ms": now_ms.saturating_add(one_second_ms)
        }),
    );
    let gate_session_path = write_dummy_json(
        &case_dir,
        "entry-readiness-gate-session.json",
        valid_entry_readiness_gate_session_json(),
    );
    let financial_envelope_path = write_dummy_json(
        &case_dir,
        "financial-envelope.json",
        serde_json::json!({"record_kind": "test_financial_envelope"}),
    );
    let pre_run_state_path = write_dummy_json(
        &case_dir,
        "pre-run-state.json",
        serde_json::json!({"record_kind": "test_pre_run_state"}),
    );
    let abort_plan_path = write_dummy_json(
        &case_dir,
        "abort-plan.json",
        serde_json::json!({"record_kind": "test_abort_plan"}),
    );
    let canary_evidence_path = case_dir.join("canary-evidence.json");
    let approval_nonce_path = write_dummy_json(
        &case_dir,
        "approval-nonce.json",
        serde_json::json!({"record_kind": "test_approval_nonce"}),
    );
    let approval_consumption_path = case_dir.join("approval-consumption.json");
    let decision_evidence_path = case_dir.join("decision-evidence.jsonl");
    write_valid_decision_evidence_chain(&decision_evidence_path, now_ms, "0.01");
    let ssm_manifest_sha256 = sha256_file(&ssm_manifest_path);
    let strategy_input_evidence_sha256 = sha256_file(&strategy_input_evidence_path);
    let expected_gate_session_sha256 = sha256_file(&gate_session_path);
    let financial_envelope_sha256 = sha256_file(&financial_envelope_path);
    let pre_run_state_sha256 = sha256_file(&pre_run_state_path);
    let abort_plan_sha256 = sha256_file(&abort_plan_path);
    let approval_nonce_sha256 = sha256_file(&approval_nonce_path);
    let canary_evidence_path = canary_evidence_path.to_string_lossy().to_string();
    let strategy_cancel_path = case_dir
        .join("strategy-cancel.json")
        .to_string_lossy()
        .to_string();
    let root_toml_sha256 = sha256_file(&repo_path("tests/fixtures/bolt_v3/root.toml"));
    let head_sha = option_env!("BOLT_V3_BUILD_HEAD_SHA").unwrap_or_else(|| {
        panic!(
            "BOLT_V3_BUILD_HEAD_SHA is not compiled in; \
             run tests from a git repository so build.rs can emit the SHA"
        )
    });
    let approval_envelope_path = write_dummy_json(
        &case_dir,
        "approval-envelope.json",
        serde_json::json!({
            "schema_version": 1,
            "record_kind": "phase8_operator_approval_envelope",
            "head_sha": head_sha,
            "ssm_manifest_sha256": ssm_manifest_sha256,
            "strategy_input_evidence_sha256": strategy_input_evidence_sha256,
            "financial_envelope_sha256": financial_envelope_sha256,
            "pre_run_state_sha256": pre_run_state_sha256,
            "abort_plan_sha256": abort_plan_sha256,
            "approval_id_hash": sha256_hex("operator-approved-canary-001".as_bytes()),
            "approval_nonce_sha256": approval_nonce_sha256,
            "approval_not_before_unix_secs": approval_not_before_unix_seconds,
            "approval_not_after_unix_secs": approval_not_after_unix_seconds,
            "canary_evidence_path_hash": sha256_hex(canary_evidence_path.as_bytes()),
            "strategy_cancel_path_hash": sha256_hex(strategy_cancel_path.as_bytes()),
        }),
    );
    let approval_envelope_sha256 = sha256_file(&approval_envelope_path);
    let approval_consumption_proof = serde_json::json!({
        "schema_version": APPROVAL_CONSUMPTION_SCHEMA_VERSION,
        "record_kind": APPROVAL_CONSUMPTION_RECORD_KIND,
        "head_sha": head_sha,
        "root_toml_sha256": root_toml_sha256,
        "approval_envelope_sha256": approval_envelope_sha256,
        "ssm_manifest_sha256": ssm_manifest_sha256,
        "strategy_input_evidence_sha256": strategy_input_evidence_sha256,
        "financial_envelope_sha256": financial_envelope_sha256,
        "pre_run_state_sha256": pre_run_state_sha256,
        "abort_plan_sha256": abort_plan_sha256,
        "approval_id_hash": sha256_hex("operator-approved-canary-001".as_bytes()),
        "approval_nonce_sha256": approval_nonce_sha256,
        "approval_not_before_unix_secs": approval_not_before_unix_seconds,
        "approval_not_after_unix_secs": approval_not_after_unix_seconds,
        "canary_evidence_path_hash": sha256_hex(canary_evidence_path.as_bytes()),
        "strategy_cancel_path_hash": sha256_hex(strategy_cancel_path.as_bytes()),
        "consumed_unix_secs": now,
    });
    fs::write(
        &approval_consumption_path,
        serde_json::to_vec(&approval_consumption_proof)
            .expect("approval consumption proof JSON should encode"),
    )
    .expect("approval consumption proof should be written");

    LiveCanaryOperatorEvidenceBlock {
        head_sha: head_sha.to_string(),
        max_operator_evidence_file_bytes: 4096,
        approval_consumption_max_age_seconds: 300,
        approval_envelope_path: approval_envelope_path.to_string_lossy().to_string(),
        approval_envelope_sha256,
        ssm_manifest_path: ssm_manifest_path.to_string_lossy().to_string(),
        ssm_manifest_sha256,
        strategy_input_evidence_path: strategy_input_evidence_path.to_string_lossy().to_string(),
        strategy_input_evidence_sha256,
        gate_session_path: Some(gate_session_path.to_string_lossy().to_string()),
        expected_gate_session_sha256: Some(expected_gate_session_sha256),
        financial_envelope_path: financial_envelope_path.to_string_lossy().to_string(),
        financial_envelope_sha256,
        pre_run_state_path: pre_run_state_path.to_string_lossy().to_string(),
        pre_run_state_sha256,
        abort_plan_path: abort_plan_path.to_string_lossy().to_string(),
        abort_plan_sha256,
        canary_proof_candidate_source_path: None,
        canary_proof_candidate_source_sha256: None,
        canary_proof_order_intent_path: None,
        canary_proof_order_intent_sha256: None,
        canary_evidence_path,
        approval_not_before_unix_seconds,
        approval_not_after_unix_seconds,
        approval_nonce_path: approval_nonce_path.to_string_lossy().to_string(),
        approval_nonce_sha256,
        approval_consumption_path: approval_consumption_path.to_string_lossy().to_string(),
        decision_evidence_path: decision_evidence_path.to_string_lossy().to_string(),
        nt_submit_event_path: case_dir
            .join("nt-submit-event.json")
            .to_string_lossy()
            .to_string(),
        venue_order_state_path: case_dir
            .join("venue-order-state.json")
            .to_string_lossy()
            .to_string(),
        strategy_cancel_path: Some(strategy_cancel_path),
        restart_reconciliation_path: case_dir
            .join("restart-reconciliation.json")
            .to_string_lossy()
            .to_string(),
        post_run_hygiene_path: case_dir
            .join("post-run-hygiene.json")
            .to_string_lossy()
            .to_string(),
    }
}

fn write_valid_decision_evidence_chain(path: &Path, now_ms: u64, notional: &str) {
    use bolt_v2::bolt_v3_decision_evidence::{
        BoltV3AdmissionDecisionEvidence, BoltV3AdmissionOutcome, BoltV3GateEvidenceIdentity,
        BoltV3OrderIntentEvidence, BoltV3OrderIntentKind, BoltV3OrderIntentOrderFields,
        BoltV3StrategyInputEvidenceSnapshot,
    };
    use bolt_v2::bolt_v3_submit_admission::BoltV3SubmitIntentKind;

    let mut gate_evidence = BTreeMap::new();
    gate_evidence.insert(
        "decision_reference".to_string(),
        BoltV3GateEvidenceIdentity {
            satisfaction_kind: "evidence".to_string(),
            selected_market_key: "b".repeat(64),
            provider_id: Some("resolution_oracle_primary".to_string()),
            provider_kind: Some("chainlink_data_streams".to_string()),
            value_kind: Some("price".to_string()),
            normalized_value_sha256: Some("c".repeat(64)),
            provider_provenance_sha256: Some("d".repeat(64)),
            resolution_identity: None,
            artifact_sha256s: vec!["e".repeat(64)],
        },
    );
    gate_evidence.insert(
        "resolution".to_string(),
        BoltV3GateEvidenceIdentity {
            satisfaction_kind: "no_resolution".to_string(),
            selected_market_key: "b".repeat(64),
            provider_id: None,
            provider_kind: None,
            value_kind: None,
            normalized_value_sha256: None,
            provider_provenance_sha256: None,
            resolution_identity: Some("configured-reference-price".to_string()),
            artifact_sha256s: Vec::new(),
        },
    );
    let snapshot = BoltV3StrategyInputEvidenceSnapshot {
        strategy_id: "binary_oracle_edge_taker-001".to_string(),
        configured_target_id: "configured_updown_target".to_string(),
        market_selection_ruleset_id: "configured_updown_target".to_string(),
        gate_session_hash: "a".repeat(64),
        selected_market_key: "b".repeat(64),
        gate_evidence,
        market_selection_outcome: "current".to_string(),
        market_id: Some("configured-market".to_string()),
        polymarket_condition_id: Some("configured-condition".to_string()),
        polymarket_market_slug: Some("configured-market".to_string()),
        polymarket_question_id: Some("configured-question".to_string()),
        up_instrument_id: Some("configured-condition-UP.POLYMARKET".to_string()),
        down_instrument_id: Some("configured-condition-DOWN.POLYMARKET".to_string()),
        market_selection_timestamp_ms: Some(now_ms),
        selected_market_observed_timestamp_ms: Some(now_ms),
        polymarket_market_start_timestamp_ms: Some(now_ms),
        polymarket_market_end_timestamp_ms: Some(now_ms.saturating_add(60_000)),
        price_to_beat_source: "configured-reference-price".to_string(),
        price_to_beat_value: "3100".to_string(),
        reference_quote_ts_event: now_ms,
        spot_price: "3101".to_string(),
        reference_fair_value: Some("3101".to_string()),
        realized_volatility: "1.5".to_string(),
        seconds_to_market_end: 60,
        pricing_kurtosis: "3".to_string(),
        theta_decay_factor: "1".to_string(),
        theta_scaled_min_edge_bps: "100".to_string(),
        fair_probability_up: "0.5".to_string(),
        uncertainty_band_probability: "0.1".to_string(),
        expected_edge_basis_points: "200".to_string(),
        worst_case_edge_basis_points: "150".to_string(),
        fee_rate_basis_points: "10".to_string(),
        selected_side: Some("up".to_string()),
        submission_instrument_id: "configured-condition-UP.POLYMARKET".to_string(),
        submission_order_side: "BUY".to_string(),
        submission_price: "0.50".to_string(),
        submission_quantity: "1.0".to_string(),
        client_order_id: "configured-client-order".to_string(),
    };
    let intent = BoltV3OrderIntentEvidence {
        strategy_id: snapshot.strategy_id.clone(),
        intent_kind: BoltV3OrderIntentKind::Entry,
        instrument_id: snapshot.submission_instrument_id.clone(),
        client_order_id: snapshot.client_order_id.clone(),
        order_side: snapshot.submission_order_side.clone(),
        price: snapshot.submission_price.clone(),
        quantity: snapshot.submission_quantity.clone(),
        order_fields: BoltV3OrderIntentOrderFields {
            order_type: "LIMIT".to_string(),
            time_in_force: "FOK".to_string(),
            price: Some(snapshot.submission_price.clone()),
            trigger_price: None,
            activation_price: None,
            trigger_type: None,
            trigger_instrument_id: None,
            trailing_offset: None,
            trailing_offset_type: None,
            expire_time_unix_nanos: None,
            is_post_only: false,
            is_reduce_only: false,
            is_quote_quantity: false,
        },
    };
    let admission = BoltV3AdmissionDecisionEvidence {
        strategy_id: snapshot.strategy_id.clone(),
        client_order_id: snapshot.client_order_id.clone(),
        instrument_id: snapshot.submission_instrument_id.clone(),
        notional: notional.to_string(),
        intent_kind: BoltV3SubmitIntentKind::Entry,
        outcome: BoltV3AdmissionOutcome::RejectedNotArmed,
    };
    let lines = [
        serde_json::json!({
            "schema_version": 5,
            "recorded_at_utc_ns": 1_i64,
            "gate_id": "bolt_v3.strategy_input_snapshot",
            "gate_version": "0.1.0",
            "kind": "strategy_input_snapshot",
            "snapshot": snapshot,
        }),
        serde_json::json!({
            "schema_version": 5,
            "recorded_at_utc_ns": 2_i64,
            "gate_id": "bolt_v3.order_intent",
            "gate_version": "0.1.0",
            "kind": "order_intent",
            "intent": intent,
        }),
        serde_json::json!({
            "schema_version": 5,
            "recorded_at_utc_ns": 3_i64,
            "gate_id": "bolt_v3.submit_admission",
            "gate_version": "0.1.0",
            "kind": "admission_decision",
            "decision": admission,
        }),
    ];
    let mut jsonl = String::new();
    for line in lines {
        jsonl.push_str(&serde_json::to_string(&line).expect("decision evidence should encode"));
        jsonl.push('\n');
    }
    fs::write(path, jsonl).expect("decision evidence should write");
}

pub fn valid_entry_readiness_gate_session_json() -> serde_json::Value {
    let selected_market_key = "b".repeat(64);
    serde_json::json!({
        "schema_version": 1,
        "record_kind": "bolt_v3.entry_readiness_gate_session.v1",
        "strategy_instance_id": "configured_updown_main",
        "configured_target_id": "configured_updown_target",
        "selected_market": {
            "configured_target_id": "configured_updown_target",
            "venue": "polymarket",
            "family_key": "updown",
            "market_id": "configured-condition",
            "instrument_ids": ["configured-condition-DOWN.POLYMARKET", "configured-condition-UP.POLYMARKET"],
            "market_class": "binary_option",
            "resolution_kind": "price",
            "resolution_identity": "configured-reference-price",
            "value_kind": "scalar_price",
            "metadata_provenance_sha256": "f".repeat(64),
            "selected_market_key": selected_market_key,
            "selected_at_ms": 1234567890_u64
        },
        "created_at_ms": 1234567890_u64,
        "satisfied_roles": {
            "decision_reference": {
                "satisfaction_kind": "evidence",
                "evidence": {
                    "schema_version": 1,
                    "record_kind": "bolt_v3.gate_evidence.v1",
                    "role": "decision_reference",
                    "provider_id": "resolution_oracle_primary",
                    "provider_kind": "chainlink_data_streams",
                    "selected_market_key": selected_market_key,
                    "collector_observed_at_ms": 1234567890_u64,
                    "source_observed_at_ms": 1234567890_u64,
                    "fresh_until_ms": 1234568490_u64,
                    "value_kind": "price",
                    "normalized_value": {"price": "3101"},
                    "normalized_value_sha256": "c".repeat(64),
                    "provider_provenance": {"source": "test"},
                    "provider_provenance_sha256": "d".repeat(64),
                    "artifact_refs": [{"path": "reference-source.json", "sha256": "e".repeat(64)}]
                }
            },
            "resolution": {
                "satisfaction_kind": "no_resolution",
                "selected_market_key": selected_market_key,
                "resolution_identity": "configured-reference-price"
            }
        },
        "session_hash": "a".repeat(64),
        "artifact_refs": []
    })
}

fn live_canary_operator_evidence_case_dir() -> PathBuf {
    static ROOT: OnceLock<tempfile::TempDir> = OnceLock::new();
    let root = ROOT
        .get_or_init(|| tempfile::tempdir().expect("operator evidence tempdir should be created"));
    let counter = TEMP_DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
    let case_dir = root.path().join(format!("operator-evidence-{counter}"));
    fs::create_dir_all(&case_dir).expect("operator evidence case dir should be created");
    case_dir
}

fn write_dummy_json(dir: &Path, filename: &str, value: serde_json::Value) -> PathBuf {
    let path = dir.join(filename);
    fs::write(
        &path,
        serde_json::to_vec(&value).expect("dummy operator evidence JSON should encode"),
    )
    .expect("dummy operator evidence JSON should be written");
    path
}

fn sha256_file(path: &Path) -> String {
    sha256_hex(&fs::read(path).expect("dummy operator evidence should be readable"))
}

pub fn loaded_bolt_v3_live_canary_with_satisfied_report(
    max_live_order_count: u32,
    max_notional_per_order: rust_decimal::Decimal,
) -> bolt_v2::bolt_v3_config::LoadedBoltV3Config {
    let root_path = repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded =
        bolt_v2::bolt_v3_config::load_bolt_v3_config(&root_path).expect("fixture should load");
    let temp = TempCaseDir::new("bolt-v3-validated-gate-report");
    let temp_path = temp.persist();
    loaded.root.persistence.catalog_directory = temp_path.to_string_lossy().to_string();
    loaded.root.risk.default_max_notional_per_order = max_notional_per_order.to_string();
    let report_path = temp_path.join("no-submit-readiness.json");
    write_satisfied_no_submit_readiness_report(&report_path, &loaded.config_bundle_checksum);
    let readiness_report_max_age_seconds = 60;
    loaded.root.live_canary = Some(bolt_v2::bolt_v3_config::LiveCanaryBlock {
        approval_id: "operator-approved-canary-001".to_string(),
        no_submit_readiness_report_path: report_path.to_string_lossy().to_string(),
        max_live_order_count,
        max_notional_per_order: max_notional_per_order.to_string(),
        max_no_submit_readiness_report_bytes: 4096,
        readiness_report_max_age_seconds,
        reference_quote_max_age_seconds: readiness_report_max_age_seconds,
        reference_quote_wait_timeout_seconds: 10,
        reference_quote_probe_actor_id: "no-submit-reference-quote-probe".to_string(),
        reference_quote_probe_log_events: true,
        reference_quote_probe_log_commands: true,
        egress_identity_observed_path: None,
        egress_identity_observed_max_bytes: None,
        approved_egress_identity_sha256: None,
        proof_policy: None,
        operator_evidence: Some(valid_live_canary_operator_evidence()),
    });
    loaded
}

pub fn validated_bolt_v3_live_canary_gate_report(
    max_live_order_count: u32,
    max_notional_per_order: rust_decimal::Decimal,
) -> bolt_v2::bolt_v3_live_canary_gate::BoltV3LiveCanaryGateReport {
    let loaded = loaded_bolt_v3_live_canary_with_satisfied_report(
        max_live_order_count,
        max_notional_per_order,
    );

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime should build");
    runtime
        .block_on(bolt_v2::bolt_v3_live_canary_gate::check_bolt_v3_live_canary_gate(&loaded))
        .expect("valid live canary fixture should pass gate")
}

fn write_satisfied_no_submit_readiness_report(path: &Path, config_bundle_checksum: &str) {
    use bolt_v2::bolt_v3_no_submit_readiness_schema::{
        CONTROLLED_CONNECT_STAGE, CONTROLLED_DISCONNECT_STAGE, GENERATED_AT_UNIX_SECONDS_KEY,
        LIVE_NODE_BUILD_STAGE, NO_SUBMIT_READINESS_SCHEMA_VERSION, OPERATOR_APPROVAL_STAGE,
        REFERENCE_READINESS_STAGE, REPORT_WRITE_STAGE, SCHEMA_VERSION_KEY, SECRET_RESOLUTION_STAGE,
    };

    let report = serde_json::json!({
        SCHEMA_VERSION_KEY: NO_SUBMIT_READINESS_SCHEMA_VERSION,
        "approval_id_hash": sha256_hex("operator-approved-canary-001".as_bytes()),
        "executable_identity": current_executable_identity(),
        "config_bundle_checksum": config_bundle_checksum,
        GENERATED_AT_UNIX_SECONDS_KEY: current_unix_seconds(),
        "stages": [
            { "stage": OPERATOR_APPROVAL_STAGE, "status": "satisfied" },
            { "stage": SECRET_RESOLUTION_STAGE, "status": "satisfied" },
            { "stage": LIVE_NODE_BUILD_STAGE, "status": "satisfied" },
            { "stage": CONTROLLED_CONNECT_STAGE, "status": "satisfied" },
            { "stage": REFERENCE_READINESS_STAGE, "status": "satisfied" },
            { "stage": CONTROLLED_DISCONNECT_STAGE, "status": "satisfied" },
            { "stage": REPORT_WRITE_STAGE, "status": "satisfied" }
        ]
    });
    fs::write(
        path,
        serde_json::to_vec(&report).expect("report JSON should encode"),
    )
    .expect("readiness report should be written");
}

fn current_unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("test system clock should be after UNIX_EPOCH")
        .as_secs()
}

fn current_executable_identity() -> String {
    let path = std::env::current_exe().expect("current test executable path should resolve");
    sha256_hex(&fs::read(path).expect("current test executable should be readable"))
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

impl Drop for TempCaseDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[derive(Debug)]
pub struct MockDataClientConfig {
    client_id: String,
    venue: String,
    connect_delay: Duration,
    connect_failure: Option<String>,
    disconnect_delay: Duration,
    disconnect_failure: Option<String>,
}

impl MockDataClientConfig {
    pub fn new(client_id: &str, venue: &str) -> Self {
        Self {
            client_id: client_id.to_string(),
            venue: venue.to_string(),
            connect_delay: Duration::ZERO,
            connect_failure: None,
            disconnect_delay: Duration::ZERO,
            disconnect_failure: None,
        }
    }

    pub fn with_connect_delay_ms(mut self, milliseconds: u64) -> Self {
        self.connect_delay = Duration::from_millis(milliseconds);
        self
    }

    /// Configures the mock to surface an `Err(...)` from its
    /// `DataClient::connect` implementation. The pinned NT
    /// `DataEngine::connect` swallows the error and logs it, so the
    /// client's `is_connected()` flag stays false; controlled-connect
    /// callers see this through `kernel.check_engines_connected()`
    /// returning false after dispatch returns.
    pub fn with_connect_failure(mut self, message: &str) -> Self {
        self.connect_failure = Some(message.to_string());
        self
    }

    /// Configures the mock to sleep for the given number of
    /// milliseconds inside `DataClient::disconnect` before flipping
    /// its `connected` flag. Used to drive the bolt-v3
    /// controlled-disconnect timeout path without touching real I/O.
    pub fn with_disconnect_delay_ms(mut self, milliseconds: u64) -> Self {
        self.disconnect_delay = Duration::from_millis(milliseconds);
        self
    }

    /// Configures the mock to surface an `Err(...)` from its
    /// `DataClient::disconnect` implementation. The bolt-v3
    /// controlled-disconnect boundary must propagate this as
    /// `DisconnectFailed` rather than silently swallowing it.
    pub fn with_disconnect_failure(mut self, message: &str) -> Self {
        self.disconnect_failure = Some(message.to_string());
        self
    }
}

impl ClientConfig for MockDataClientConfig {
    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[derive(Debug)]
pub struct MockExecClientConfig {
    client_id: String,
    account_id: String,
    venue: String,
}

impl MockExecClientConfig {
    pub fn new(client_id: &str, account_id: &str, venue: &str) -> Self {
        Self {
            client_id: client_id.to_string(),
            account_id: account_id.to_string(),
            venue: venue.to_string(),
        }
    }
}

impl ClientConfig for MockExecClientConfig {
    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[derive(Debug)]
pub struct MockDataClientFactory;

impl DataClientFactory for MockDataClientFactory {
    fn create(
        &self,
        _name: &str,
        config: &dyn ClientConfig,
        _cache: CacheView,
        _clock: Rc<RefCell<dyn Clock>>,
    ) -> anyhow::Result<Box<dyn DataClient>> {
        let cfg = config
            .as_any()
            .downcast_ref::<MockDataClientConfig>()
            .ok_or_else(|| anyhow::anyhow!("MockDataClientFactory received wrong config type"))?;

        Ok(Box::new(MockDataClient::new(
            ClientId::from(cfg.client_id.as_str()),
            Venue::from(cfg.venue.as_str()),
            cfg.connect_delay,
            cfg.connect_failure.clone(),
            cfg.disconnect_delay,
            cfg.disconnect_failure.clone(),
        )))
    }

    fn name(&self) -> &str {
        "mock-data"
    }

    fn config_type(&self) -> &str {
        "MockDataClientConfig"
    }
}

#[derive(Debug)]
pub struct MockExecutionClientFactory;

impl ExecutionClientFactory for MockExecutionClientFactory {
    fn create(
        &self,
        _name: &str,
        config: &dyn ClientConfig,
        _cache: CacheView,
    ) -> anyhow::Result<Box<dyn ExecutionClient>> {
        let cfg = config
            .as_any()
            .downcast_ref::<MockExecClientConfig>()
            .ok_or_else(|| {
                anyhow::anyhow!("MockExecutionClientFactory received wrong config type")
            })?;

        Ok(Box::new(MockExecutionClient::new(
            ClientId::from(cfg.client_id.as_str()),
            AccountId::from(cfg.account_id.as_str()),
            Venue::from(cfg.venue.as_str()),
            OmsType::Netting,
        )))
    }

    fn name(&self) -> &str {
        "mock-exec"
    }

    fn config_type(&self) -> &str {
        "MockExecClientConfig"
    }
}

#[derive(Debug)]
struct MockDataClient {
    client_id: ClientId,
    venue: Venue,
    connected: bool,
    connect_delay: Duration,
    connect_failure: Option<String>,
    disconnect_delay: Duration,
    disconnect_failure: Option<String>,
}

impl MockDataClient {
    fn new(
        client_id: ClientId,
        venue: Venue,
        connect_delay: Duration,
        connect_failure: Option<String>,
        disconnect_delay: Duration,
        disconnect_failure: Option<String>,
    ) -> Self {
        Self {
            client_id,
            venue,
            connected: false,
            connect_delay,
            connect_failure,
            disconnect_delay,
            disconnect_failure,
        }
    }
}

#[derive(Debug)]
struct MockExecutionClient {
    client_id: ClientId,
    account_id: AccountId,
    venue: Venue,
    oms_type: OmsType,
    connected: bool,
}

impl MockExecutionClient {
    fn new(client_id: ClientId, account_id: AccountId, venue: Venue, oms_type: OmsType) -> Self {
        Self {
            client_id,
            account_id,
            venue,
            oms_type,
            connected: false,
        }
    }
}

#[async_trait(?Send)]
impl DataClient for MockDataClient {
    fn client_id(&self) -> ClientId {
        self.client_id
    }

    fn venue(&self) -> Option<Venue> {
        Some(self.venue)
    }

    fn start(&mut self) -> anyhow::Result<()> {
        self.connected = true;
        Ok(())
    }

    fn stop(&mut self) -> anyhow::Result<()> {
        self.connected = false;
        Ok(())
    }

    fn reset(&mut self) -> anyhow::Result<()> {
        self.connected = false;
        Ok(())
    }

    fn dispose(&mut self) -> anyhow::Result<()> {
        self.connected = false;
        Ok(())
    }

    fn is_connected(&self) -> bool {
        self.connected
    }

    fn is_disconnected(&self) -> bool {
        !self.connected
    }

    async fn connect(&mut self) -> anyhow::Result<()> {
        if !self.connect_delay.is_zero() {
            tokio::time::sleep(self.connect_delay).await;
        }
        if let Some(message) = &self.connect_failure {
            return Err(anyhow::anyhow!(message.clone()));
        }
        self.connected = true;
        Ok(())
    }

    async fn disconnect(&mut self) -> anyhow::Result<()> {
        if !self.disconnect_delay.is_zero() {
            tokio::time::sleep(self.disconnect_delay).await;
        }
        if let Some(message) = &self.disconnect_failure {
            return Err(anyhow::anyhow!(message.clone()));
        }
        self.connected = false;
        Ok(())
    }

    fn subscribe_instrument(&mut self, cmd: SubscribeInstrument) -> anyhow::Result<()> {
        mock_data_subscriptions()
            .lock()
            .unwrap()
            .push(cmd.instrument_id.to_string());
        Ok(())
    }

    fn subscribe_quotes(&mut self, cmd: SubscribeQuotes) -> anyhow::Result<()> {
        mock_data_subscriptions()
            .lock()
            .unwrap()
            .push(cmd.instrument_id.to_string());
        Ok(())
    }

    fn subscribe_trades(&mut self, cmd: SubscribeTrades) -> anyhow::Result<()> {
        mock_data_subscriptions()
            .lock()
            .unwrap()
            .push(cmd.instrument_id.to_string());
        Ok(())
    }
}

#[async_trait(?Send)]
impl ExecutionClient for MockExecutionClient {
    fn is_connected(&self) -> bool {
        self.connected
    }

    fn client_id(&self) -> ClientId {
        self.client_id
    }

    fn account_id(&self) -> AccountId {
        self.account_id
    }

    fn venue(&self) -> Venue {
        self.venue
    }

    fn oms_type(&self) -> OmsType {
        self.oms_type
    }

    fn get_account(&self) -> Option<AccountAny> {
        None
    }

    fn generate_account_state(
        &self,
        _balances: Vec<AccountBalance>,
        _margins: Vec<MarginBalance>,
        _reported: bool,
        _ts_event: nautilus_core::UnixNanos,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    fn start(&mut self) -> anyhow::Result<()> {
        self.connected = true;
        Ok(())
    }

    fn stop(&mut self) -> anyhow::Result<()> {
        self.connected = false;
        Ok(())
    }

    async fn connect(&mut self) -> anyhow::Result<()> {
        self.connected = true;
        Ok(())
    }

    async fn disconnect(&mut self) -> anyhow::Result<()> {
        self.connected = false;
        Ok(())
    }

    fn submit_order(&self, cmd: SubmitOrder) -> anyhow::Result<()> {
        mock_exec_submissions()
            .lock()
            .unwrap()
            .push(RecordedSubmitOrder {
                client_id: cmd.client_id,
                strategy_id: cmd.strategy_id,
                instrument_id: cmd.instrument_id,
                client_order_id: cmd.client_order_id,
            });
        Ok(())
    }
}

/// PKCS8-wrapped Ed25519 private key, base64-encoded. The bolt-v3 Binance
/// provider validator requires that the resolved api_secret decode as a
/// valid PKCS8 Ed25519 key, so the fake resolver must hand back a value
/// that satisfies it.
const FAKE_BOLT_V3_BINANCE_API_SECRET: &str =
    "MC4CAQAwBQYDK2VwBCIEIAABAgMEBQYHCAkKCwwNDg8QERITFBUWFxgZGhscHR4f";

/// 32-byte secp256k1 private key in hex (with the `0x` prefix the NT
/// Polymarket adapter accepts). The NT `PolymarketExecutionClient::new`
/// constructor parses this into an EVM signer at registration time, so
/// the fake resolver must hand back a value that decodes to a valid
/// secp256k1 scalar; the all-`0x42` byte sequence is well within the
/// curve order and is shared across bolt-v3 build-path tests.
const FAKE_BOLT_V3_POLYMARKET_PRIVATE_KEY: &str =
    "0x4242424242424242424242424242424242424242424242424242424242424242";

/// Synthetic SSM resolver for bolt-v3 LiveNode build tests. Returns
/// per-path placeholder values that satisfy the polymarket and binance
/// secret schemas declared in `tests/fixtures/bolt_v3/root.toml` so the
/// build path can run all the way through `LiveNodeBuilder::build`
/// (which invokes the real NT `factory.create` for every registered
/// client) without reaching the network. The polymarket private key
/// must be a valid 32-byte secp256k1 hex value because NT's
/// `PolymarketExecutionClient::new` parses it into a signer; the
/// polymarket api_secret must be valid base64 because NT's
/// `Credential::new` decodes it into HMAC key material.
pub fn fake_bolt_v3_resolver(_region: &str, path: &str) -> Result<String, &'static str> {
    match path {
        "/bolt/polymarket_main/private_key" => Ok(FAKE_BOLT_V3_POLYMARKET_PRIVATE_KEY.to_string()),
        "/bolt/polymarket_main/api_key" => Ok("polymarket-api-key".to_string()),
        "/bolt/polymarket_main/api_secret" => Ok("YWJj".to_string()),
        "/bolt/polymarket_main/passphrase" => Ok("polymarket-passphrase".to_string()),
        "/bolt/binance_reference/api_key" => Ok("binance-api-key".to_string()),
        "/bolt/binance_reference/api_secret" => Ok(FAKE_BOLT_V3_BINANCE_API_SECRET.to_string()),
        _ => Err("unexpected SSM path requested by bolt-v3 fake resolver"),
    }
}
