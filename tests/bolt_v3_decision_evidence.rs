mod support;

use std::sync::Arc;

use anyhow::Result;
use bolt_v2::{
    bolt_v3_config::load_bolt_v3_config,
    bolt_v3_decision_evidence::{
        BOLT_V3_DECISION_EVIDENCE_GATE_VERSION, BOLT_V3_DECISION_EVIDENCE_SCHEMA_VERSION,
        BOLT_V3_ORDER_INTENT_GATE_ID, BOLT_V3_STRATEGY_INPUT_SNAPSHOT_GATE_ID,
        BOLT_V3_SUBMIT_ADMISSION_GATE_ID, BoltV3AdmissionDecisionEvidence, BoltV3AdmissionOutcome,
        BoltV3DecisionEvidenceWriter, BoltV3OrderIntentEvidence, BoltV3OrderIntentKind,
        BoltV3OrderIntentOrderFields, BoltV3StrategyInputEvidenceSnapshot, BoltV3SubmitIntentKind,
        decision_evidence_path, read_latest_entry_decision_evidence_chain,
    },
    strategies::registry::FeeProvider,
    strategies::registry::StrategyBuildContext,
};
use futures_util::future::{BoxFuture, FutureExt};
use nautilus_model::enums::{OrderSide, OrderType, TimeInForce};
use nautilus_model::identifiers::InstrumentId;
use rust_decimal::Decimal;

struct NoopFeeProvider;

impl FeeProvider for NoopFeeProvider {
    fn fee_bps(&self, _instrument_id: InstrumentId) -> Option<Decimal> {
        None
    }

    fn warm(&self, _instrument_id: InstrumentId) -> BoxFuture<'_, Result<()>> {
        async { Ok(()) }.boxed()
    }
}

#[test]
fn latest_entry_decision_evidence_chain_binds_snapshot_order_intent_and_admission() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let evidence_path = temp.path().join("decision-evidence.jsonl");
    let lines = sample_entry_decision_evidence_lines();
    write_decision_evidence_lines(&evidence_path, &lines);

    let chain = read_latest_entry_decision_evidence_chain(&evidence_path, 100_000)
        .expect("complete entry decision evidence chain should parse");

    assert_eq!(chain.snapshot.client_order_id, "client-order-one");
    assert_eq!(chain.intent.client_order_id, chain.snapshot.client_order_id);
    assert_eq!(
        chain.admission.client_order_id,
        chain.snapshot.client_order_id
    );
}

#[test]
fn latest_entry_decision_evidence_chain_rejects_untrusted_record_metadata() {
    let cases: [(&str, fn(&mut serde_json::Value)); 8] = [
        ("missing schema_version", |line: &mut serde_json::Value| {
            line.as_object_mut()
                .expect("line should be an object")
                .remove("schema_version");
        }),
        ("wrong schema_version", |line: &mut serde_json::Value| {
            line["schema_version"] =
                serde_json::json!(BOLT_V3_DECISION_EVIDENCE_SCHEMA_VERSION + 1);
        }),
        (
            "missing recorded_at_utc_ns",
            |line: &mut serde_json::Value| {
                line.as_object_mut()
                    .expect("line should be an object")
                    .remove("recorded_at_utc_ns");
            },
        ),
        (
            "nonpositive recorded_at_utc_ns",
            |line: &mut serde_json::Value| {
                line["recorded_at_utc_ns"] = serde_json::json!(0_i64);
            },
        ),
        ("missing gate_id", |line: &mut serde_json::Value| {
            line.as_object_mut()
                .expect("line should be an object")
                .remove("gate_id");
        }),
        ("wrong gate_id", |line: &mut serde_json::Value| {
            line["gate_id"] = serde_json::json!("bolt_v3.wrong_gate");
        }),
        ("missing gate_version", |line: &mut serde_json::Value| {
            line.as_object_mut()
                .expect("line should be an object")
                .remove("gate_version");
        }),
        ("wrong gate_version", |line: &mut serde_json::Value| {
            line["gate_version"] = serde_json::json!("wrong-version");
        }),
    ];

    for (case_name, mutate) in cases {
        let temp = tempfile::tempdir().expect("tempdir should create");
        let evidence_path = temp.path().join("decision-evidence.jsonl");
        let mut lines = sample_entry_decision_evidence_lines();
        mutate(&mut lines[0]);
        write_decision_evidence_lines(&evidence_path, &lines);

        let error = read_latest_entry_decision_evidence_chain(&evidence_path, 100_000)
            .expect_err(case_name);

        assert!(
            error.to_string().contains("decision evidence"),
            "{case_name} should fail as decision evidence metadata; got {error:#}"
        );
    }
}

#[test]
fn latest_entry_decision_evidence_chain_rejects_oversized_file_before_parse() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let evidence_path = temp.path().join("decision-evidence.jsonl");
    let lines = sample_entry_decision_evidence_lines();
    write_decision_evidence_lines(&evidence_path, &lines);

    let error = read_latest_entry_decision_evidence_chain(&evidence_path, 8)
        .expect_err("bounded decision evidence reader must reject oversized input");

    assert!(
        error.to_string().contains("exceeds max_bytes=8"),
        "oversized decision evidence should name byte bound: {error:#}"
    );
}

#[test]
fn latest_entry_decision_evidence_chain_rejects_cross_record_field_mismatches() {
    let cases: [(&str, fn(&mut [serde_json::Value; 3])); 7] = [
        ("intent strategy_id", |lines| {
            lines[1]["intent"]["strategy_id"] = serde_json::json!("other-strategy");
        }),
        ("admission strategy_id", |lines| {
            lines[2]["decision"]["strategy_id"] = serde_json::json!("other-strategy");
        }),
        ("intent instrument_id", |lines| {
            lines[1]["intent"]["instrument_id"] = serde_json::json!("other-instrument");
        }),
        ("admission instrument_id", |lines| {
            lines[2]["decision"]["instrument_id"] = serde_json::json!("other-instrument");
        }),
        ("order_side", |lines| {
            lines[1]["intent"]["order_side"] = serde_json::json!("Sell");
        }),
        ("price", |lines| {
            lines[1]["intent"]["price"] = serde_json::json!("0.51");
        }),
        ("quantity", |lines| {
            lines[1]["intent"]["quantity"] = serde_json::json!("2");
        }),
    ];

    for (field, mutate) in cases {
        let temp = tempfile::tempdir().expect("tempdir should create");
        let evidence_path = temp.path().join("decision-evidence.jsonl");
        let mut lines = sample_entry_decision_evidence_lines();
        mutate(&mut lines);
        write_decision_evidence_lines(&evidence_path, &lines);

        let error =
            read_latest_entry_decision_evidence_chain(&evidence_path, 100_000).expect_err(field);

        assert!(
            error
                .to_string()
                .contains(field.split_whitespace().last().expect("field label")),
            "{field} mismatch should be diagnostic: {error:#}"
        );
    }
}

fn sample_entry_decision_evidence_lines() -> [serde_json::Value; 3] {
    let snapshot = BoltV3StrategyInputEvidenceSnapshot {
        strategy_id: "strategy-one".to_string(),
        configured_target_id: "target-one".to_string(),
        market_selection_ruleset_id: "target-one".to_string(),
        market_selection_outcome: "current".to_string(),
        market_id: Some("market-one".to_string()),
        polymarket_condition_id: Some("condition-one".to_string()),
        polymarket_market_slug: Some("market-slug-one".to_string()),
        polymarket_question_id: Some("question-one".to_string()),
        up_instrument_id: Some("instrument-up".to_string()),
        down_instrument_id: Some("instrument-down".to_string()),
        market_selection_timestamp_ms: Some(1000),
        selected_market_observed_timestamp_ms: Some(1000),
        polymarket_market_start_timestamp_ms: Some(1000),
        polymarket_market_end_timestamp_ms: Some(301000),
        price_to_beat_source: "source-one".to_string(),
        price_to_beat_value: "3100".to_string(),
        reference_quote_ts_event: 1200,
        spot_price: "3100.5".to_string(),
        reference_fair_value: Some("3100.5".to_string()),
        realized_volatility: "1.5".to_string(),
        seconds_to_market_end: 300,
        pricing_kurtosis: "0".to_string(),
        theta_decay_factor: "0".to_string(),
        theta_scaled_min_edge_bps: "10".to_string(),
        fair_probability_up: "0.6".to_string(),
        uncertainty_band_probability: "0.01".to_string(),
        expected_edge_basis_points: "10".to_string(),
        worst_case_edge_basis_points: "10".to_string(),
        fee_rate_basis_points: "0".to_string(),
        selected_side: Some("up".to_string()),
        submission_instrument_id: "instrument-up".to_string(),
        submission_order_side: OrderSide::Buy.to_string(),
        submission_price: "0.50".to_string(),
        submission_quantity: "1".to_string(),
        client_order_id: "client-order-one".to_string(),
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
            order_type: OrderType::Limit.to_string(),
            time_in_force: TimeInForce::Gtc.to_string(),
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
        notional: "0.50".to_string(),
        intent_kind: BoltV3SubmitIntentKind::Entry,
        outcome: BoltV3AdmissionOutcome::RejectedNotArmed,
    };
    let lines = [
        serde_json::json!({
            "schema_version": BOLT_V3_DECISION_EVIDENCE_SCHEMA_VERSION,
            "recorded_at_utc_ns": 1_i64,
            "gate_id": BOLT_V3_STRATEGY_INPUT_SNAPSHOT_GATE_ID,
            "gate_version": BOLT_V3_DECISION_EVIDENCE_GATE_VERSION,
            "kind": "strategy_input_snapshot",
            "snapshot": snapshot,
        }),
        serde_json::json!({
            "schema_version": BOLT_V3_DECISION_EVIDENCE_SCHEMA_VERSION,
            "recorded_at_utc_ns": 2_i64,
            "gate_id": BOLT_V3_ORDER_INTENT_GATE_ID,
            "gate_version": BOLT_V3_DECISION_EVIDENCE_GATE_VERSION,
            "kind": "order_intent",
            "intent": intent,
        }),
        serde_json::json!({
            "schema_version": BOLT_V3_DECISION_EVIDENCE_SCHEMA_VERSION,
            "recorded_at_utc_ns": 3_i64,
            "gate_id": BOLT_V3_SUBMIT_ADMISSION_GATE_ID,
            "gate_version": BOLT_V3_DECISION_EVIDENCE_GATE_VERSION,
            "kind": "admission_decision",
            "decision": admission,
        }),
    ];
    lines
}

fn write_decision_evidence_lines(path: &std::path::Path, lines: &[serde_json::Value]) {
    let mut body = String::new();
    for line in lines {
        body.push_str(&serde_json::to_string(&line).expect("line should serialize"));
        body.push('\n');
    }
    std::fs::write(path, body).expect("decision evidence should write");
}

#[derive(Debug)]
struct NoopDecisionEvidenceWriter;

impl BoltV3DecisionEvidenceWriter for NoopDecisionEvidenceWriter {
    fn record_strategy_input_snapshot(
        &self,
        _snapshot: &BoltV3StrategyInputEvidenceSnapshot,
    ) -> Result<()> {
        Ok(())
    }

    fn record_order_intent(&self, _intent: &BoltV3OrderIntentEvidence) -> Result<()> {
        Ok(())
    }

    fn record_admission_decision(&self, _decision: &BoltV3AdmissionDecisionEvidence) -> Result<()> {
        Ok(())
    }
}

#[test]
fn decision_evidence_path_stays_under_configured_catalog_directory() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let temp = support::TempCaseDir::new("bolt-v3-decision-evidence-path");
    loaded.root.persistence.catalog_directory = temp.path().to_string_lossy().to_string();

    let path = decision_evidence_path(&loaded).expect("fixture evidence path should resolve");

    assert!(path.starts_with(temp.path()));
    assert_eq!(
        path.strip_prefix(temp.path()).unwrap(),
        std::path::Path::new("bolt-v3/decision-evidence/order-intents.jsonl")
    );
}

#[test]
fn decision_evidence_path_rejects_absolute_or_parent_traversal() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");

    for invalid in ["/tmp/order-intents.jsonl", "../order-intents.jsonl"] {
        loaded
            .root
            .persistence
            .decision_evidence
            .order_intents_relative_path = invalid.to_string();
        let error = decision_evidence_path(&loaded)
            .expect_err("invalid decision evidence path should be rejected");
        assert!(
            error
                .to_string()
                .contains("order_intents_relative_path must be non-empty, relative"),
            "unexpected error for {invalid}: {error:#}"
        );
    }
}

#[test]
fn binary_oracle_edge_taker_records_evidence_then_admission_before_only_direct_submit_call() {
    let source = include_str!("../src/strategies/binary_oracle_edge_taker.rs");
    let evidence_index = source
        .find(".record_order_intent(&intent)")
        .expect("strategy must record decision evidence");
    let admission_index = source
        .find(".submit_admission().admit(&request)")
        .expect("strategy wrapper must submit through admission");
    let submit_index = source
        .find(
            "self.submit_order(\n            order,\n            submit_context.position_id,\n            submit_context.client_id,\n            submit_context.params,\n        )",
        )
        .expect("strategy wrapper must thread submit context into the only direct NT submit call");

    assert!(
        evidence_index < admission_index && admission_index < submit_index,
        "decision evidence must be recorded before submit admission before NT submit"
    );
    let strategy_input_index = source
        .find(".record_strategy_input_snapshot(&strategy_input_snapshot)")
        .expect("entry strategy input snapshot must be recorded");
    let evidence_wrapper_call_after_strategy_input = source[strategy_input_index..]
        .find("self.submit_order_with_decision_evidence(\n                    intent,\n                    order,\n                    SubmitContext::with_client_id(client_id),\n                )")
        .expect("entry path must submit through evidence wrapper");
    assert!(
        evidence_wrapper_call_after_strategy_input > 0,
        "entry strategy input snapshot must be recorded before order-intent evidence wrapper"
    );
    // This intentionally scans the whole strategy source, including in-file
    // tests, because no code path should bypass the evidence wrapper.
    assert_eq!(
        source.matches("self.submit_order(").count(),
        1,
        "direct NT submit calls must stay inside evidence wrapper only"
    );
}

#[test]
fn binary_oracle_edge_taker_exit_submit_threads_managed_position_id_to_nt() {
    let source = include_str!("../src/strategies/binary_oracle_edge_taker.rs");

    assert!(
        source.contains(
            "SubmitContext::with_client_id_and_position_id(\n                client_id,\n                managed_position.position.position_id,\n            )"
        ),
        "exit submits must pass the managed PositionId into NT submit_order"
    );
}

#[test]
fn strategy_build_context_requires_decision_evidence_value() {
    let context = StrategyBuildContext::new(
        Arc::new(NoopFeeProvider),
        Arc::new(NoopDecisionEvidenceWriter),
        Arc::new(
            bolt_v2::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new_unarmed(Arc::new(
                NoopDecisionEvidenceWriter,
            )),
        ),
    );

    assert!(
        context
            .decision_evidence()
            .record_order_intent(&BoltV3OrderIntentEvidence {
                strategy_id: "strategy-a".to_string(),
                intent_kind: BoltV3OrderIntentKind::Entry,
                instrument_id: "instrument-a".to_string(),
                client_order_id: "order-a".to_string(),
                order_side: OrderSide::Buy.to_string(),
                price: "0.50".to_string(),
                quantity: "1".to_string(),
                order_fields: BoltV3OrderIntentOrderFields {
                    order_type: OrderType::Limit.to_string(),
                    time_in_force: TimeInForce::Gtc.to_string(),
                    price: Some("0.50".to_string()),
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
            })
            .is_ok()
    );
}
