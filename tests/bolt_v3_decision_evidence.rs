use crate::support;

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use anyhow::Result;
use bolt_v2::bolt_v3_strategy_context::StrategyBuildContext;

use bolt_v2::{
    bolt_v3_config::load_bolt_v3_config,
    bolt_v3_decision_evidence::{
        BOLT_V3_DECISION_EVIDENCE_GATE_VERSION, BOLT_V3_DECISION_EVIDENCE_SCHEMA_VERSION,
        BOLT_V3_ENTRY_SKIP_GATE_ID, BOLT_V3_EXIT_DECISION_GATE_ID, BOLT_V3_EXIT_EVALUATION_GATE_ID,
        BOLT_V3_EXIT_EVALUATION_RECORD_KIND, BOLT_V3_LOSS_GOVERNOR_HALT_GATE_ID,
        BOLT_V3_LOSS_GOVERNOR_HALT_RECORD_KIND, BOLT_V3_ORDER_INTENT_GATE_ID,
        BOLT_V3_ORDER_REJECT_GATE_ID, BOLT_V3_ORDER_REJECT_RECORD_KIND,
        BOLT_V3_REQUOTE_THROTTLE_GATE_ID, BOLT_V3_SETTLEMENT_BOOKING_ERROR_RECORD_KIND,
        BOLT_V3_SETTLEMENT_GATE_ID, BOLT_V3_SETTLEMENT_RECORD_KIND,
        BOLT_V3_STRATEGY_INPUT_SNAPSHOT_GATE_ID, BOLT_V3_SUBMIT_ADMISSION_GATE_ID,
        BOLT_V3_TERMINAL_SETTLEMENT_RECORD_KIND, BoltV3AdmissionDecisionEvidence,
        BoltV3AdmissionOutcome, BoltV3BasketAdmissionDecisionEvidence,
        BoltV3BasketAdmissionOutcome, BoltV3CapitalAdmissionRebuildAuditEvidence,
        BoltV3DecisionEvidenceWriter, BoltV3EntryBlockReason, BoltV3EntryPricingBlockReason,
        BoltV3EntrySkipEvidence, BoltV3EntrySkipReasonCategory, BoltV3ExitDecisionEvidence,
        BoltV3ExitDecisionOutcome, BoltV3ExitEvaluationEvidence, BoltV3ExitRvGateResult,
        BoltV3ExitRvSnapshotBlocker, BoltV3ExitTriggerSource, BoltV3ForcedFlatReason,
        BoltV3LossGovernorHaltEvidence, BoltV3LossSnapshotSource, BoltV3OrderIntentEvidence,
        BoltV3OrderIntentKind, BoltV3OrderIntentOrderFields, BoltV3OrderLifecycleEvidence,
        BoltV3OrderLifecycleOutcome, BoltV3OrderLifecycleTransition, BoltV3OrderRejectEvidence,
        BoltV3OrderRejectReason, BoltV3OutcomeSide,
        BoltV3RealizedVolatilitySourceDiagnosticEvidence, BoltV3RejectSource,
        BoltV3RequoteActionCostClass, BoltV3RequoteThrottleBlockReason, BoltV3RequoteThrottleBound,
        BoltV3RequoteThrottleEvidence, BoltV3RvGateResult, BoltV3SettlementBookingErrorEvidence,
        BoltV3SettlementBookingErrorReason, BoltV3SettlementEvidence, BoltV3StaleLossReason,
        BoltV3StrategyInputEvidenceSnapshot, BoltV3SubmitIntentKind,
        BoltV3SubmitReservationFillEvidence, BoltV3SubmitReservationMetadataEvidence,
        BoltV3TerminalSettlementEvidence, JsonlBoltV3DecisionEvidenceWriter,
        decision_evidence_path, read_exit_evaluation_evidence,
        read_latest_entry_decision_evidence_chain, read_loss_governor_halt_evidence,
        read_order_reject_evidence, read_settlement_booking_error_evidence,
        read_settlement_booking_error_keys_for_recovery_scope, read_settlement_evidence,
        read_settlement_evidence_for_recovery_scope, read_settlement_keys_for_recovery_scope,
        read_submit_reservation_recovery_evidence, read_terminal_settlement_evidence,
    },
    bolt_v3_fair_value_pricing::classify_rv_gate,
    bolt_v3_realized_volatility::{
        RealizedVolAggregation, RealizedVolBlockReason, RealizedVolPricingComponent,
        RealizedVolSampleKind, RealizedVolSnapshot, RealizedVolSourceClass,
        RealizedVolSourceDiagnostic, RealizedVolSourceRejectReason, RealizedVolSourceStatus,
    },
    bolt_v3_timestamp_domain::LocalReceiveMs,
};
use nautilus_model::enums::{OrderSide, OrderType, TimeInForce};

const EXPECTED_CAPITAL_ADMISSION_RECOVERY_SCHEMA_VERSION: u32 = 15;

#[test]
fn decision_evidence_schema_version_tracks_reference_price_and_capital_admission_records() {
    assert_eq!(
        BOLT_V3_DECISION_EVIDENCE_SCHEMA_VERSION,
        EXPECTED_CAPITAL_ADMISSION_RECOVERY_SCHEMA_VERSION
    );
}

#[test]
fn strategy_input_evidence_records_realized_volatility_snapshot_provenance() {
    let snapshot = strategy_input_snapshot_with_realized_volatility_snapshot();

    assert_eq!(snapshot.realized_volatility_surface_id, "<surface_id>");
    assert_eq!(snapshot.realized_volatility_annualized_decimal, "2.5");
    assert_eq!(snapshot.realized_volatility_aggregation, "upper_quantile");
    assert_eq!(
        snapshot.realized_volatility_sources_used,
        vec!["<SOURCE_ID_A>".to_string()]
    );
    assert!(snapshot.realized_volatility_blockers.is_empty());
    assert_eq!(
        snapshot.up_worst_case_edge_basis_points.as_deref(),
        Some("11")
    );
    assert_eq!(
        snapshot.down_worst_case_edge_basis_points.as_deref(),
        Some("9")
    );
    assert_eq!(
        snapshot.pricing_blocked_by,
        vec![BoltV3EntryPricingBlockReason::RealizedVolNotReady]
    );
    assert_eq!(snapshot.fast_venue_name.as_deref(), Some("fast-source"));
    assert_eq!(snapshot.fast_venue_age_ms, Some(20));
    assert_eq!(snapshot.fast_venue_jitter_ms, Some(3));
    assert!(!snapshot.fast_venue_incoherent);
    assert_eq!(snapshot.lead_agreement_corr.as_deref(), Some("0.98"));
}

#[test]
fn realized_volatility_source_diagnostic_evidence_exports_config_participation() {
    let diagnostic = RealizedVolSourceDiagnostic {
        source_id: "<SOURCE_ID_B>".to_string(),
        source_class: RealizedVolSourceClass::SpotQuote,
        sample_kind: RealizedVolSampleKind::Midpoint,
        enabled: false,
        counts_toward_quorum: false,
        status: RealizedVolSourceStatus::DiagnosticOnly,
        annualized_realized_vol_decimal: None,
        measured_annualized_realized_vol_decimal: None,
        noise_robust_annualized_realized_vol_decimal: None,
        continuous_annualized_realized_vol_decimal: None,
        jump_annualized_realized_vol_decimal: None,
        first_sample_ts_ms: None,
        last_sample_ts_ms: None,
        raw_sample_count: 0,
        grid_sample_count: 0,
        coverage_ratio: 0.0,
        max_inter_sample_gap_ms: None,
        last_rejected_reason: Some(RealizedVolSourceRejectReason::DisabledSource),
        last_rejected_event_ts_ms: None,
        last_rejected_recv_ts_ms: None,
        rejection_counters: BTreeMap::from([(RealizedVolSourceRejectReason::DisabledSource, 2)]),
        block_reason: Some(RealizedVolBlockReason::NotWarm),
    };

    let evidence =
        BoltV3RealizedVolatilitySourceDiagnosticEvidence::from_realized_vol_diagnostic(&diagnostic);

    assert_eq!(evidence.source_id, "<SOURCE_ID_B>");
    assert!(!evidence.enabled);
    assert!(!evidence.counts_toward_quorum);
    assert_eq!(evidence.status, "diagnostic_only");
    assert_eq!(
        evidence.rejection_counters.get("disabled_source").copied(),
        Some(2)
    );
}

fn strategy_input_snapshot_with_realized_volatility_snapshot() -> BoltV3StrategyInputEvidenceSnapshot
{
    BoltV3StrategyInputEvidenceSnapshot {
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
        fast_venue_available: true,
        reference_current_price: Some("3100.5".to_string()),
        reference_current_price_available: true,
        reference_current_price_source_id: Some("chainlink_primary".to_string()),
        reference_current_price_failed_over: Some(false),
        realized_volatility: "2.5".to_string(),
        realized_volatility_surface_id: "<surface_id>".to_string(),
        realized_volatility_as_of_ms: Some(1200),
        realized_volatility_gate_result: Some(BoltV3RvGateResult::Accepted),
        realized_volatility_receive_watermark_ms: Some(LocalReceiveMs::new(1_199)),
        realized_volatility_annualized_decimal: "2.5".to_string(),
        realized_volatility_measured_annualized_decimal: "2.5".to_string(),
        realized_volatility_noise_robust_annualized_decimal: "2.4".to_string(),
        realized_volatility_continuous_annualized_decimal: "2.3".to_string(),
        realized_volatility_jump_annualized_decimal: "0.1".to_string(),
        realized_volatility_forecast_annualized_decimal: String::new(),
        realized_volatility_pricing_component: "noise_robust".to_string(),
        realized_volatility_seconds_per_annum: "31536000".to_string(),
        realized_volatility_aggregation: "upper_quantile".to_string(),
        realized_volatility_sources_used: vec!["<SOURCE_ID_A>".to_string()],
        realized_volatility_source_diagnostics: Vec::new(),
        realized_volatility_unknown_source_rejections: BTreeMap::new(),
        realized_volatility_blockers: Vec::new(),
        realized_volatility_config_fingerprint: "<config_fingerprint>".to_string(),
        seconds_to_market_end: 300,
        pricing_kurtosis: "0".to_string(),
        theta_decay_factor: "0".to_string(),
        theta_scaled_min_edge_bps: "10".to_string(),
        fair_probability_up: "0.6".to_string(),
        uncertainty_band_probability: "0.01".to_string(),
        expected_edge_basis_points: "10".to_string(),
        worst_case_edge_basis_points: "10".to_string(),
        up_worst_case_edge_basis_points: Some("11".to_string()),
        down_worst_case_edge_basis_points: Some("9".to_string()),
        gate_blocked_by: Vec::new(),
        pricing_blocked_by: vec![BoltV3EntryPricingBlockReason::RealizedVolNotReady],
        fast_venue_name: Some("fast-source".to_string()),
        fast_venue_age_ms: Some(20),
        fast_venue_jitter_ms: Some(3),
        fast_venue_incoherent: false,
        lead_agreement_corr: Some("0.98".to_string()),
        selected_side: Some("up".to_string()),
        submission_instrument_id: "instrument-up".to_string(),
        submission_order_side: OrderSide::Buy.to_string(),
        submission_price: "0.50".to_string(),
        submission_quantity: "1".to_string(),
        client_order_id: "client-order-one".to_string(),
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
fn latest_entry_decision_evidence_chain_skips_basket_admission_records() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let evidence_path = temp.path().join("decision-evidence.jsonl");
    let mut lines = sample_entry_decision_evidence_lines().to_vec();
    lines.insert(1, sample_basket_admission_decision_line());
    write_decision_evidence_lines(&evidence_path, &lines);

    let chain = read_latest_entry_decision_evidence_chain(&evidence_path, 100_000)
        .expect("basket admission decisions must not block entry-chain recovery");

    assert_eq!(chain.snapshot.client_order_id, "client-order-one");
}

#[test]
#[allow(clippy::type_complexity)]
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

#[cfg(unix)]
#[test]
fn latest_entry_decision_evidence_chain_rejects_symlinked_file_before_parse() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let evidence_path = temp.path().join("decision-evidence.jsonl");
    let real_path = temp.path().join("real-decision-evidence.jsonl");
    let lines = sample_entry_decision_evidence_lines();
    write_decision_evidence_lines(&real_path, &lines);
    std::os::unix::fs::symlink(&real_path, &evidence_path)
        .expect("decision evidence symlink should create");

    let error = read_latest_entry_decision_evidence_chain(&evidence_path, 100_000)
        .expect_err("symlinked decision evidence must fail before parse");
    let message = error.to_string();
    let chain = format!("{error:#}");

    assert!(
        message.contains("regular file"),
        "symlinked decision evidence should cite regular-file policy: {message}"
    );
    assert!(
        !message.contains(evidence_path.to_string_lossy().as_ref()),
        "symlinked decision evidence diagnostic must not print source path: {message}"
    );
    assert!(
        !chain.contains(evidence_path.to_string_lossy().as_ref()),
        "symlinked decision evidence error chain must not print source path: {chain}"
    );
}

#[test]
#[allow(clippy::type_complexity)]
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

#[test]
fn latest_entry_decision_evidence_chain_rejects_legacy_schema_before_admission_payload_parse() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let evidence_path = temp.path().join("decision-evidence.jsonl");
    let mut lines = sample_entry_decision_evidence_lines();
    lines[2]["schema_version"] = serde_json::json!(5_u32);
    lines[2]["decision"]
        .as_object_mut()
        .expect("admission decision should be an object")
        .remove("execution_client_id");
    write_decision_evidence_lines(&evidence_path, &lines);

    let error = read_latest_entry_decision_evidence_chain(&evidence_path, 100_000)
        .expect_err("legacy decision evidence should fail closed before payload parsing");
    let rendered = format!("{error:#}");
    assert!(
        rendered.contains("schema_version mismatch"),
        "legacy schema should fail on envelope schema, got: {rendered}"
    );
    assert!(
        !rendered.contains("execution_client_id"),
        "legacy schema should not reach current admission payload parsing, got: {rendered}"
    );
}

#[test]
fn submit_reservation_recovery_rejects_noncanonical_metadata_encodings() {
    for (field, value) in [("side", "Buy"), ("product_kind", "PredictionMarketBinary")] {
        let temp = tempfile::tempdir().expect("tempdir should create");
        let evidence_path = temp.path().join("decision-evidence.jsonl");
        let mut metadata = sample_submit_reservation_metadata();
        match field {
            "side" => metadata.side = value.to_string(),
            "product_kind" => metadata.product_kind = value.to_string(),
            _ => unreachable!("test only mutates known fields"),
        }
        write_decision_evidence_lines(
            &evidence_path,
            &[serde_json::json!({
                "schema_version": BOLT_V3_DECISION_EVIDENCE_SCHEMA_VERSION,
                "recorded_at_utc_ns": 1_i64,
                "gate_id": BOLT_V3_SUBMIT_ADMISSION_GATE_ID,
                "gate_version": BOLT_V3_DECISION_EVIDENCE_GATE_VERSION,
                "kind": "submit_reservation_metadata",
                "metadata": metadata,
            })],
        );

        let error = read_submit_reservation_recovery_evidence(&evidence_path, 100_000)
            .expect_err("non-canonical submit reservation metadata must fail at read time");
        let rendered = format!("{error:#}");
        assert!(
            rendered.contains(field) && rendered.contains("canonical"),
            "expected canonical {field} diagnostic, got: {rendered}"
        );
    }
}

#[test]
fn submit_reservation_recovery_skips_legacy_v9_non_recovery_lines() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let evidence_path = temp.path().join("decision-evidence.jsonl");
    let mut lines = sample_entry_decision_evidence_lines().to_vec();
    for line in &mut lines {
        line["schema_version"] = serde_json::json!(BOLT_V3_DECISION_EVIDENCE_SCHEMA_VERSION - 1);
    }
    lines.push(serde_json::json!({
        "schema_version": EXPECTED_CAPITAL_ADMISSION_RECOVERY_SCHEMA_VERSION,
        "recorded_at_utc_ns": 4_i64,
        "gate_id": BOLT_V3_SUBMIT_ADMISSION_GATE_ID,
        "gate_version": BOLT_V3_DECISION_EVIDENCE_GATE_VERSION,
        "kind": "submit_reservation_metadata",
        "metadata": sample_submit_reservation_metadata(),
    }));
    write_decision_evidence_lines(&evidence_path, &lines);

    let recovery = read_submit_reservation_recovery_evidence(&evidence_path, 100_000)
        .expect("legacy v9 non-recovery lines must not block reservation recovery");

    assert!(
        recovery
            .metadata_by_client_order_id
            .contains_key("client-order-one"),
        "current reservation metadata should recover despite legacy non-recovery lines"
    );
}

#[test]
fn submit_reservation_recovery_skips_older_schema_admission_before_payload_parse() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let evidence_path = temp.path().join("decision-evidence.jsonl");
    let mut legacy_admission = sample_entry_decision_evidence_lines()[2].clone();
    legacy_admission["schema_version"] =
        serde_json::json!(BOLT_V3_DECISION_EVIDENCE_SCHEMA_VERSION - 1);
    legacy_admission["decision"]
        .as_object_mut()
        .expect("legacy admission decision should be an object")
        .remove("execution_client_id");
    write_decision_evidence_lines(
        &evidence_path,
        &[
            legacy_admission,
            serde_json::json!({
                "schema_version": BOLT_V3_DECISION_EVIDENCE_SCHEMA_VERSION,
                "recorded_at_utc_ns": 4_i64,
                "gate_id": BOLT_V3_SUBMIT_ADMISSION_GATE_ID,
                "gate_version": BOLT_V3_DECISION_EVIDENCE_GATE_VERSION,
                "kind": "submit_reservation_metadata",
                "metadata": sample_submit_reservation_metadata(),
            }),
            serde_json::json!({
                "schema_version": BOLT_V3_DECISION_EVIDENCE_SCHEMA_VERSION,
                "recorded_at_utc_ns": 5_i64,
                "gate_id": BOLT_V3_SUBMIT_ADMISSION_GATE_ID,
                "gate_version": BOLT_V3_DECISION_EVIDENCE_GATE_VERSION,
                "kind": "submit_reservation_fill",
                "fill": sample_submit_reservation_fill(),
            }),
        ],
    );

    let recovery = read_submit_reservation_recovery_evidence(&evidence_path, 100_000)
        .expect("older-schema admission lines must not block reservation recovery");
    let recovered = recovery
        .metadata_by_client_order_id
        .get("client-order-one")
        .expect("current reservation metadata should recover");

    assert_eq!(
        recovered.metadata.submit_reservation_id,
        "client-order-one#1"
    );
    assert_eq!(recovered.fill_trade_ids.len(), 1);
    assert!(
        recovered.fill_trade_ids.contains("trade-one"),
        "current reservation fill should recover with the metadata"
    );
}

#[test]
fn submit_reservation_recovery_skips_basket_admission_records() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let evidence_path = temp.path().join("decision-evidence.jsonl");
    write_decision_evidence_lines(
        &evidence_path,
        &[
            sample_basket_admission_decision_line(),
            serde_json::json!({
                "schema_version": EXPECTED_CAPITAL_ADMISSION_RECOVERY_SCHEMA_VERSION,
                "recorded_at_utc_ns": 2_i64,
                "gate_id": BOLT_V3_SUBMIT_ADMISSION_GATE_ID,
                "gate_version": BOLT_V3_DECISION_EVIDENCE_GATE_VERSION,
                "kind": "submit_reservation_metadata",
                "metadata": sample_submit_reservation_metadata(),
            }),
        ],
    );

    let recovery = read_submit_reservation_recovery_evidence(&evidence_path, 100_000)
        .expect("basket admission decisions must not block submit-reservation recovery");

    assert!(
        recovery
            .metadata_by_client_order_id
            .contains_key("client-order-one")
    );
}

#[test]
fn submit_reservation_recovery_skips_below_current_schema_audit_only_records() {
    for mut legacy_audit_line in [
        sample_basket_admission_decision_line(),
        serde_json::json!({
            "schema_version": BOLT_V3_DECISION_EVIDENCE_SCHEMA_VERSION,
            "recorded_at_utc_ns": 1_i64,
            "gate_id": BOLT_V3_ENTRY_SKIP_GATE_ID,
            "gate_version": BOLT_V3_DECISION_EVIDENCE_GATE_VERSION,
            "kind": "entry_skip",
            "entry_skip": sample_entry_skip_evidence(),
        }),
        serde_json::json!({
            "schema_version": BOLT_V3_DECISION_EVIDENCE_SCHEMA_VERSION,
            "recorded_at_utc_ns": 1_i64,
            "gate_id": BOLT_V3_EXIT_DECISION_GATE_ID,
            "gate_version": BOLT_V3_DECISION_EVIDENCE_GATE_VERSION,
            "kind": "exit_decision",
            "exit_decision": sample_exit_decision_evidence(),
        }),
        serde_json::json!({
            "schema_version": BOLT_V3_DECISION_EVIDENCE_SCHEMA_VERSION,
            "recorded_at_utc_ns": 1_i64,
            "gate_id": BOLT_V3_REQUOTE_THROTTLE_GATE_ID,
            "gate_version": BOLT_V3_DECISION_EVIDENCE_GATE_VERSION,
            "kind": "requote_throttle",
            "requote_throttle": sample_requote_throttle_evidence(),
        }),
    ] {
        let kind = legacy_audit_line["kind"]
            .as_str()
            .expect("audit line should carry a kind")
            .to_string();
        legacy_audit_line["schema_version"] =
            serde_json::json!(BOLT_V3_DECISION_EVIDENCE_SCHEMA_VERSION - 1);
        let temp = tempfile::tempdir().expect("tempdir should create");
        let evidence_path = temp.path().join("decision-evidence.jsonl");
        let metadata = sample_submit_reservation_metadata();
        let client_order_id = metadata.client_order_id.clone();
        write_decision_evidence_lines(
            &evidence_path,
            &[
                legacy_audit_line,
                serde_json::json!({
                    "schema_version": BOLT_V3_DECISION_EVIDENCE_SCHEMA_VERSION,
                    "recorded_at_utc_ns": 2_i64,
                    "gate_id": BOLT_V3_SUBMIT_ADMISSION_GATE_ID,
                    "gate_version": BOLT_V3_DECISION_EVIDENCE_GATE_VERSION,
                    "kind": "submit_reservation_metadata",
                    "metadata": metadata,
                }),
            ],
        );

        let recovery = read_submit_reservation_recovery_evidence(&evidence_path, 100_000)
            .unwrap_or_else(|error| {
                panic!("{kind} below-current audit line must not block recovery: {error:#}")
            });

        assert!(
            recovery
                .metadata_by_client_order_id
                .contains_key(&client_order_id),
            "{kind} below-current audit line should allow current reservation metadata recovery"
        );
    }
}

#[test]
fn entry_skip_evidence_writes_one_durable_line_and_readers_skip_it() {
    let (_temp, evidence_path, writer) = temp_decision_evidence_writer("entry-skip");
    let evidence = sample_entry_skip_evidence();

    writer
        .record_entry_skip(&evidence)
        .expect("entry skip evidence should write through the durable writer");

    let lines = read_decision_evidence_json_lines(&evidence_path);
    assert_eq!(lines.len(), 1);
    assert_eq!(
        lines[0]["schema_version"],
        BOLT_V3_DECISION_EVIDENCE_SCHEMA_VERSION
    );
    assert_eq!(lines[0]["kind"], "entry_skip");
    let decoded: BoltV3EntrySkipEvidence =
        serde_json::from_value(lines[0]["entry_skip"].clone()).expect("entry skip should decode");
    assert_eq!(decoded, evidence);
    assert_eq!(
        decoded.reason_category,
        BoltV3EntrySkipReasonCategory::EntryPricingBlocked
    );
    assert_eq!(decoded.market_id.as_deref(), Some("market-one"));
    assert_eq!(decoded.sized_worst_case_ev_bps.as_deref(), Some("12.5"));

    append_decision_evidence_lines(&evidence_path, &sample_entry_decision_evidence_lines());
    read_latest_entry_decision_evidence_chain(&evidence_path, 100_000)
        .expect("entry skip record must not block entry-chain recovery");
    read_submit_reservation_recovery_evidence(&evidence_path, 100_000)
        .expect("entry skip record must not block submit-reservation recovery");
}

#[test]
fn entry_skip_admitted_markers_default_false_for_predeploy_lines() {
    let mut value = serde_json::to_value(sample_entry_skip_evidence())
        .expect("entry skip evidence should encode as json");
    value
        .as_object_mut()
        .expect("entry skip should encode as an object")
        .remove("fast_venue_available");
    value
        .as_object_mut()
        .expect("entry skip should encode as an object")
        .remove("reference_current_price_available");

    let decoded: BoltV3EntrySkipEvidence =
        serde_json::from_value(value).expect("predeploy entry skip should decode");

    assert!(!decoded.fast_venue_available);
    assert!(!decoded.reference_current_price_available);
}

#[test]
fn strategy_input_snapshot_admitted_markers_default_false_for_predeploy_lines() {
    let mut value =
        serde_json::to_value(strategy_input_snapshot_with_realized_volatility_snapshot())
            .expect("strategy input snapshot should encode as json");
    value
        .as_object_mut()
        .expect("strategy input snapshot should encode as an object")
        .remove("fast_venue_available");
    value
        .as_object_mut()
        .expect("strategy input snapshot should encode as an object")
        .remove("reference_current_price_available");

    let decoded: BoltV3StrategyInputEvidenceSnapshot =
        serde_json::from_value(value).expect("predeploy strategy input snapshot should decode");

    assert!(!decoded.fast_venue_available);
    assert!(!decoded.reference_current_price_available);
}

#[test]
fn probability_wire_fields_remain_string_payload_bytes() {
    let entry_skip = sample_entry_skip_evidence();
    let entry_skip_bytes =
        serde_json::to_string(&entry_skip).expect("entry skip evidence should serialize");

    assert_eq!(
        entry_skip_bytes,
        concat!(
            r#"{"strategy_id":"strategy-one","now_ms":1200,"reason_category":"entry_pricing_blocked","#,
            r#""unclassified_context":null,"gate_blocked_by":[{"forced_flat":"stale_reference"}],"#,
            r#""pricing_blocked_by":["realized_vol_not_ready"],"market_id":"market-one","#,
            r#""phase":"Active","seconds_to_market_end":300,"spot_price":"3100.5","#,
            r#""reference_current_price":"3100.5","fast_venue_available":true,"#,
            r#""reference_current_price_available":true,"realized_vol":"2.5","#,
            r#""realized_vol_source_venue":"fast-source","realized_vol_source_ts_ms":1100,"#,
            r#""realized_vol_gate_result":"accepted","realized_vol_receive_watermark_ms":1099,"#,
            r#""realized_vol_snapshot":null,"#,
            r#""fair_probability_up":"0.6","fair_probability_down":"0.4","selected_side":"up","#,
            r#""sized_notional":"25","sized_worst_case_ev_bps":"12.5","#,
            r#""sized_edge_cents_per_share":"1.25","theta_scaled_min_edge_bps":"10","#,
            r#""submission_blocked_reason":"entry_pricing_blocked","stale_reference_after_ms":1500,"#,
            r#""last_reference_ts_ms":1000,"min_liquidity_required":"100","#,
            r#""liquidity_available":"80","frozen":false,"metadata_matches_selection":true,"#,
            r#""fast_venue_incoherent":false}"#,
        )
    );

    let snapshot_line = sample_entry_decision_evidence_lines()[0].clone();
    let snapshot = &snapshot_line["snapshot"];
    assert_eq!(snapshot["fair_probability_up"], serde_json::json!("0.6"));
    assert_eq!(
        snapshot["uncertainty_band_probability"],
        serde_json::json!("0.01")
    );
    assert!(
        snapshot["fair_probability_up"].is_string()
            && snapshot["uncertainty_band_probability"].is_string()
    );
}

#[test]
fn exit_decision_evidence_writes_one_durable_line_and_readers_skip_it() {
    let (_temp, evidence_path, writer) = temp_decision_evidence_writer("exit-decision");
    let evidence = sample_exit_decision_evidence();

    writer
        .record_exit_decision(&evidence)
        .expect("exit decision evidence should write through the durable writer");

    let lines = read_decision_evidence_json_lines(&evidence_path);
    assert_eq!(lines.len(), 1);
    assert_eq!(
        lines[0]["schema_version"],
        BOLT_V3_DECISION_EVIDENCE_SCHEMA_VERSION
    );
    assert_eq!(lines[0]["kind"], "exit_decision");
    let decoded: BoltV3ExitDecisionEvidence =
        serde_json::from_value(lines[0]["exit_decision"].clone())
            .expect("exit decision should decode");
    assert_eq!(decoded, evidence);
    assert_eq!(
        decoded.exit_decision,
        BoltV3ExitDecisionOutcome::ExitFailClosed
    );
    assert_eq!(
        decoded.forced_flat_reasons,
        vec![BoltV3ForcedFlatReason::StaleReference]
    );
    assert_eq!(decoded.exit_ev_bps.as_deref(), Some("3.5"));
    assert_eq!(
        lines[0]["exit_decision"]["spot_price"],
        serde_json::json!("3100.5")
    );
    assert_eq!(
        lines[0]["exit_decision"]["reference_current_price"],
        serde_json::json!("3099.75")
    );
    assert_eq!(
        lines[0]["exit_decision"]["fast_venue_available"],
        serde_json::json!(true)
    );
    assert_eq!(
        lines[0]["exit_decision"]["reference_current_price_available"],
        serde_json::json!(true)
    );
    assert_eq!(decoded.spot_venue_name.as_deref(), Some("venue-one"));
    assert_eq!(decoded.interval_open.as_deref(), Some("3100"));
    assert_eq!(decoded.fair_probability_up.as_deref(), Some("0.55"));
    assert_eq!(decoded.fair_probability_down.as_deref(), Some("0.45"));
    assert_eq!(
        decoded.uncertainty_band_probability.as_deref(),
        Some("0.02")
    );
    assert_eq!(decoded.submission_order_side.as_deref(), Some("Sell"));
    assert_eq!(decoded.submission_price.as_deref(), Some("0.49"));
    assert_eq!(decoded.submission_quantity.as_deref(), Some("1"));

    append_decision_evidence_lines(&evidence_path, &sample_entry_decision_evidence_lines());
    read_latest_entry_decision_evidence_chain(&evidence_path, 100_000)
        .expect("exit decision record must not block entry-chain recovery");
    read_submit_reservation_recovery_evidence(&evidence_path, 100_000)
        .expect("exit decision record must not block submit-reservation recovery");
}

#[test]
fn exit_decision_observed_inputs_default_absent_for_predeploy_lines() {
    let line = fixture_decision_evidence_line(
        "tests/fixtures/bolt_v3/predeploy_exit_decision_evidence.jsonl",
    );
    assert_eq!(line["kind"], "exit_decision");

    let decoded: BoltV3ExitDecisionEvidence = serde_json::from_value(line["exit_decision"].clone())
        .expect("predeploy exit decision should decode");

    assert_eq!(decoded.spot_price, None);
    assert_eq!(decoded.spot_venue_name, None);
    assert!(!decoded.fast_venue_available);
    assert_eq!(decoded.reference_current_price, None);
    assert!(!decoded.reference_current_price_available);
    assert_eq!(decoded.interval_open, None);
    assert_eq!(decoded.fair_probability_up, None);
    assert_eq!(decoded.fair_probability_down, None);
    assert_eq!(decoded.uncertainty_band_probability, None);
    assert_eq!(decoded.submission_order_side, None);
    assert_eq!(decoded.submission_price, None);
    assert_eq!(decoded.submission_quantity, None);
}

#[test]
fn requote_throttle_evidence_writes_one_durable_line_and_readers_skip_it() {
    let (_temp, evidence_path, writer) = temp_decision_evidence_writer("requote-throttle");
    let evidence = sample_requote_throttle_evidence();

    writer
        .record_requote_throttle(&evidence)
        .expect("requote throttle evidence should write through the durable writer");

    let lines = read_decision_evidence_json_lines(&evidence_path);
    assert_eq!(lines.len(), 1);
    assert_eq!(
        lines[0]["schema_version"],
        BOLT_V3_DECISION_EVIDENCE_SCHEMA_VERSION
    );
    assert_eq!(lines[0]["kind"], "requote_throttle");
    let decoded: BoltV3RequoteThrottleEvidence =
        serde_json::from_value(lines[0]["requote_throttle"].clone())
            .expect("requote throttle should decode");
    assert_eq!(decoded, evidence);
    assert_eq!(
        decoded.action_cost_class,
        BoltV3RequoteActionCostClass::FreshSubmit
    );
    assert_eq!(
        decoded.block_reason,
        BoltV3RequoteThrottleBlockReason::RequoteBudgetExhausted
    );
    assert_eq!(
        decoded.bound_by,
        BoltV3RequoteThrottleBound::SubmitCommandWindow
    );
    assert_eq!(decoded.submit_commands_in_window, 40);

    append_decision_evidence_lines(&evidence_path, &sample_entry_decision_evidence_lines());
    read_latest_entry_decision_evidence_chain(&evidence_path, 100_000)
        .expect("requote throttle record must not block entry-chain recovery");
    read_submit_reservation_recovery_evidence(&evidence_path, 100_000)
        .expect("requote throttle record must not block submit-reservation recovery");
}

#[test]
fn submit_reservation_recovery_rejects_legacy_v9_reservation_metadata() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let evidence_path = temp.path().join("decision-evidence.jsonl");
    write_decision_evidence_lines(
        &evidence_path,
        &[serde_json::json!({
            "schema_version": BOLT_V3_DECISION_EVIDENCE_SCHEMA_VERSION - 1,
            "recorded_at_utc_ns": 1_i64,
            "gate_id": BOLT_V3_SUBMIT_ADMISSION_GATE_ID,
            "gate_version": BOLT_V3_DECISION_EVIDENCE_GATE_VERSION,
            "kind": "submit_reservation_metadata",
            "metadata": sample_submit_reservation_metadata(),
        })],
    );

    // A reservation-bearing record below the current schema must FAIL CLOSED, not
    // be silently skipped: only audit-only (non-recovery) kinds are skip-eligible.
    // Failing closed degrades startup to the unreconciled gate rather than
    // silently dropping a possibly-open reservation.
    let error = read_submit_reservation_recovery_evidence(&evidence_path, 100_000)
        .expect_err("legacy v9 reservation metadata must fail closed");
    let rendered = format!("{error:#}");
    assert!(
        rendered.contains("schema_version mismatch"),
        "expected schema mismatch for legacy reservation metadata, got: {rendered}"
    );
}

fn temp_decision_evidence_writer(
    label: &str,
) -> (
    support::TempCaseDir,
    std::path::PathBuf,
    JsonlBoltV3DecisionEvidenceWriter,
) {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let temp = support::TempCaseDir::new(label);
    loaded.root.persistence.catalog_directory = temp.path().to_string_lossy().to_string();
    let path = decision_evidence_path(&loaded).expect("fixture evidence path should resolve");
    let writer = JsonlBoltV3DecisionEvidenceWriter::from_loaded_config(&loaded)
        .expect("jsonl decision evidence writer should open");
    (temp, path, writer)
}

fn read_decision_evidence_json_lines(path: &std::path::Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(path)
        .expect("decision evidence should read")
        .lines()
        .map(|line| serde_json::from_str(line).expect("decision evidence line should be json"))
        .collect()
}

#[test]
fn jsonl_decision_evidence_shutdown_drain_succeeds_after_record_write() {
    let (_temp, path, writer) = temp_decision_evidence_writer("decision-evidence-shutdown-drain");

    writer
        .record_entry_skip(&sample_entry_skip_evidence())
        .expect("entry-skip evidence should write");
    writer
        .drain_shutdown()
        .expect("shutdown drain must fail loudly only when disk sync fails");

    let lines = read_decision_evidence_json_lines(&path);
    assert_eq!(lines.len(), 1);
    assert_eq!(
        lines[0]["kind"],
        serde_json::Value::String("entry_skip".to_string())
    );
}

fn append_decision_evidence_lines(path: &std::path::Path, lines: &[serde_json::Value]) {
    use std::io::Write;

    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(path)
        .expect("decision evidence should open for append");
    for line in lines {
        writeln!(
            file,
            "{}",
            serde_json::to_string(line).expect("line should serialize")
        )
        .expect("decision evidence line should append");
    }
}

fn sample_entry_skip_evidence() -> BoltV3EntrySkipEvidence {
    BoltV3EntrySkipEvidence {
        strategy_id: "strategy-one".to_string(),
        now_ms: 1_200,
        reason_category: BoltV3EntrySkipReasonCategory::EntryPricingBlocked,
        unclassified_context: None,
        gate_blocked_by: vec![BoltV3EntryBlockReason::ForcedFlat(
            BoltV3ForcedFlatReason::StaleReference,
        )],
        pricing_blocked_by: vec![BoltV3EntryPricingBlockReason::RealizedVolNotReady],
        market_id: Some("market-one".to_string()),
        phase: "Active".to_string(),
        seconds_to_market_end: Some(300),
        spot_price: Some("3100.5".to_string()),
        reference_current_price: Some("3100.5".to_string()),
        fast_venue_available: true,
        reference_current_price_available: true,
        realized_vol: Some("2.5".to_string()),
        realized_vol_source_venue: Some("fast-source".to_string()),
        realized_vol_source_ts_ms: Some(1_100),
        realized_vol_gate_result: Some(BoltV3RvGateResult::Accepted),
        realized_vol_receive_watermark_ms: Some(LocalReceiveMs::new(1_099)),
        realized_vol_snapshot: None,
        fair_probability_up: Some("0.6".to_string()),
        fair_probability_down: Some("0.4".to_string()),
        selected_side: Some(BoltV3OutcomeSide::Up),
        sized_notional: Some("25".to_string()),
        sized_worst_case_ev_bps: Some("12.5".to_string()),
        sized_edge_cents_per_share: Some("1.25".to_string()),
        theta_scaled_min_edge_bps: Some("10".to_string()),
        submission_blocked_reason: Some(BoltV3EntrySkipReasonCategory::EntryPricingBlocked),
        stale_reference_after_ms: Some(1_500),
        last_reference_ts_ms: Some(1_000),
        min_liquidity_required: Some("100".to_string()),
        liquidity_available: Some("80".to_string()),
        frozen: false,
        metadata_matches_selection: true,
        fast_venue_incoherent: false,
    }
}

#[test]
fn entry_rv_receipt_fields_deserialize_from_legacy_evidence() {
    let mut accepted_skip =
        serde_json::to_value(sample_entry_skip_evidence()).expect("serialize skip");
    let accepted_skip_object = accepted_skip
        .as_object_mut()
        .expect("skip must serialize as object");
    accepted_skip_object.remove("realized_vol_gate_result");
    accepted_skip_object.remove("realized_vol_receive_watermark_ms");
    accepted_skip_object.remove("realized_vol_snapshot");
    let skip: BoltV3EntrySkipEvidence =
        serde_json::from_value(accepted_skip.clone()).expect("accepted legacy skip");
    assert_eq!(
        skip.realized_vol_gate_result,
        Some(BoltV3RvGateResult::Accepted)
    );
    assert_eq!(skip.realized_vol_receive_watermark_ms, None);
    assert_eq!(skip.realized_vol_snapshot, None);

    let mut zero_rv_skip = accepted_skip.clone();
    zero_rv_skip
        .as_object_mut()
        .expect("zero-RV skip must remain an object")
        .insert("realized_vol".to_string(), serde_json::json!("0"));
    let zero_rv_skip: BoltV3EntrySkipEvidence =
        serde_json::from_value(zero_rv_skip).expect("zero RV is valid legacy skip evidence");
    assert_eq!(
        zero_rv_skip.realized_vol_gate_result,
        Some(BoltV3RvGateResult::Accepted)
    );

    for invalid_wire_value in ["-1", "NaN", "inf", "-inf"] {
        let mut invalid_rv_skip = accepted_skip.clone();
        invalid_rv_skip
            .as_object_mut()
            .expect("invalid-RV skip must remain an object")
            .insert(
                "realized_vol".to_string(),
                serde_json::json!(invalid_wire_value),
            );
        let invalid_rv_skip: BoltV3EntrySkipEvidence = serde_json::from_value(invalid_rv_skip)
            .expect("invalid RV string must remain readable legacy skip evidence");
        assert_eq!(
            invalid_rv_skip.realized_vol_gate_result, None,
            "invalid legacy skip RV must not infer admission: {invalid_wire_value}"
        );
    }

    let unclassifiable_skip_object = accepted_skip
        .as_object_mut()
        .expect("skip must remain an object");
    unclassifiable_skip_object.insert(
        "realized_vol_source_venue".to_string(),
        serde_json::Value::Null,
    );
    let skip: BoltV3EntrySkipEvidence =
        serde_json::from_value(accepted_skip).expect("unclassifiable legacy skip");
    assert_eq!(skip.realized_vol_gate_result, None);

    let mut snapshot =
        serde_json::to_value(strategy_input_snapshot_with_realized_volatility_snapshot())
            .expect("serialize strategy input");
    let snapshot_object = snapshot
        .as_object_mut()
        .expect("strategy input must serialize as object");
    snapshot_object.remove("realized_volatility_gate_result");
    snapshot_object.remove("realized_volatility_receive_watermark_ms");
    let accepted_snapshot: BoltV3StrategyInputEvidenceSnapshot =
        serde_json::from_value(snapshot.clone()).expect("accepted legacy strategy input");
    assert_eq!(
        accepted_snapshot.realized_volatility_gate_result,
        Some(BoltV3RvGateResult::Accepted)
    );
    assert_eq!(
        accepted_snapshot.realized_volatility_receive_watermark_ms,
        None
    );

    let mut zero_rv_snapshot = snapshot.clone();
    zero_rv_snapshot
        .as_object_mut()
        .expect("zero-RV strategy input must remain an object")
        .insert("realized_volatility".to_string(), serde_json::json!("0"));
    let zero_rv_snapshot: BoltV3StrategyInputEvidenceSnapshot =
        serde_json::from_value(zero_rv_snapshot)
            .expect("zero RV is valid legacy strategy-input evidence");
    assert_eq!(
        zero_rv_snapshot.realized_volatility_gate_result,
        Some(BoltV3RvGateResult::Accepted)
    );

    for invalid_wire_value in ["-1", "NaN", "inf", "-inf"] {
        let mut invalid_rv_snapshot = snapshot.clone();
        invalid_rv_snapshot
            .as_object_mut()
            .expect("invalid-RV strategy input must remain an object")
            .insert(
                "realized_volatility".to_string(),
                serde_json::json!(invalid_wire_value),
            );
        let invalid_rv_snapshot: BoltV3StrategyInputEvidenceSnapshot =
            serde_json::from_value(invalid_rv_snapshot)
                .expect("invalid RV string must remain readable legacy strategy-input evidence");
        assert_eq!(
            invalid_rv_snapshot.realized_volatility_gate_result, None,
            "invalid legacy strategy-input RV must not infer admission: {invalid_wire_value}"
        );
    }

    snapshot
        .as_object_mut()
        .expect("strategy input must remain an object")
        .insert(
            "realized_volatility_sources_used".to_string(),
            serde_json::json!([]),
        );
    let snapshot: BoltV3StrategyInputEvidenceSnapshot =
        serde_json::from_value(snapshot).expect("unclassifiable legacy strategy input");
    assert_eq!(snapshot.realized_volatility_gate_result, None);
}

fn sample_exit_decision_evidence() -> BoltV3ExitDecisionEvidence {
    BoltV3ExitDecisionEvidence {
        strategy_id: "strategy-one".to_string(),
        market_id: Some("market-one".to_string()),
        position_id: Some("position-one".to_string()),
        position_instrument_id: Some("instrument-up".to_string()),
        position_outcome_side: Some(BoltV3OutcomeSide::Up),
        forced_flat_reasons: vec![BoltV3ForcedFlatReason::StaleReference],
        spot_price: Some("3100.5".to_string()),
        spot_venue_name: Some("venue-one".to_string()),
        fast_venue_available: true,
        reference_current_price: Some("3099.75".to_string()),
        reference_current_price_available: true,
        interval_open: Some("3100".to_string()),
        fair_probability_up: Some("0.55".to_string()),
        fair_probability_down: Some("0.45".to_string()),
        uncertainty_band_probability: Some("0.02".to_string()),
        hold_ev_bps: Some("2.5".to_string()),
        exit_ev_bps: Some("3.5".to_string()),
        realized_vol: None,
        realized_vol_source_venue: None,
        realized_vol_source_ts_ms: None,
        exit_eval_now_ms: 1_200,
        exit_trigger_source: BoltV3ExitTriggerSource::SignalQuote,
        trigger_ts_event_ms: 1_200,
        trigger_ts_init_ms: Some(1_201),
        rv_surface_id: "surface-one".to_string(),
        rv_snapshot_as_of_ms: Some(1_250),
        rv_snapshot_ready: true,
        rv_snapshot_has_ready_realized_vol: Some(true),
        rv_snapshot_receive_watermark_ms: Some(1_200),
        rv_max_source_age_ms: Some(500),
        rv_snapshot_blockers: vec![BoltV3ExitRvSnapshotBlocker::QuorumNotReady],
        rv_source_diagnostics: Vec::new(),
        rv_gate_result: BoltV3ExitRvGateResult::RejectedFutureDated,
        rv_future_dating_delta_ms: Some(50),
        exit_hysteresis_bps: "1".to_string(),
        exit_decision: BoltV3ExitDecisionOutcome::ExitFailClosed,
        blocked_reason: None,
        client_order_id: Some("client-order-exit".to_string()),
        submission_order_side: Some("Sell".to_string()),
        submission_price: Some("0.49".to_string()),
        submission_quantity: Some("1".to_string()),
        seconds_to_market_end: Some(240),
        ts_ms: 1_200,
        stale_reference_after_ms: Some(1_500),
        last_reference_ts_ms: Some(1_000),
        min_liquidity_required: Some("100".to_string()),
        liquidity_available: Some("80".to_string()),
        frozen: false,
        metadata_matches_selection: true,
        fast_venue_incoherent: false,
    }
}

fn sample_requote_throttle_evidence() -> BoltV3RequoteThrottleEvidence {
    BoltV3RequoteThrottleEvidence {
        strategy_id: "maker-strategy".to_string(),
        family_key: "market-one".to_string(),
        market_id: Some("market-one".to_string()),
        leg: "yes".to_string(),
        now_ms: 1_000,
        observed_at_ns: 1_000_000,
        action_cost_class: BoltV3RequoteActionCostClass::FreshSubmit,
        block_reason: BoltV3RequoteThrottleBlockReason::RequoteBudgetExhausted,
        bound_by: BoltV3RequoteThrottleBound::SubmitCommandWindow,
        submit_commands_in_window: 40,
        submit_command_cap: 40,
        submit_window_ms: 60_000,
        rest_cost_in_window: 99,
        rest_cap_per_minute: 100,
        rest_window_ms: 60_000,
        min_interval_ms: 500,
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
        fast_venue_available: true,
        reference_current_price: Some("3100.5".to_string()),
        reference_current_price_available: true,
        reference_current_price_source_id: Some("chainlink_primary".to_string()),
        reference_current_price_failed_over: Some(false),
        realized_volatility: "1.5".to_string(),
        realized_volatility_surface_id: String::new(),
        realized_volatility_as_of_ms: None,
        realized_volatility_gate_result: Some(BoltV3RvGateResult::MissingSnapshot),
        realized_volatility_receive_watermark_ms: None,
        realized_volatility_annualized_decimal: "1.5".to_string(),
        realized_volatility_measured_annualized_decimal: String::new(),
        realized_volatility_noise_robust_annualized_decimal: String::new(),
        realized_volatility_continuous_annualized_decimal: String::new(),
        realized_volatility_jump_annualized_decimal: String::new(),
        realized_volatility_forecast_annualized_decimal: String::new(),
        realized_volatility_pricing_component: String::new(),
        realized_volatility_seconds_per_annum: String::new(),
        realized_volatility_aggregation: String::new(),
        realized_volatility_sources_used: Vec::new(),
        realized_volatility_source_diagnostics: Vec::new(),
        realized_volatility_unknown_source_rejections: BTreeMap::new(),
        realized_volatility_blockers: Vec::new(),
        realized_volatility_config_fingerprint: String::new(),
        seconds_to_market_end: 300,
        pricing_kurtosis: "0".to_string(),
        theta_decay_factor: "0".to_string(),
        theta_scaled_min_edge_bps: "10".to_string(),
        fair_probability_up: "0.6".to_string(),
        uncertainty_band_probability: "0.01".to_string(),
        expected_edge_basis_points: "10".to_string(),
        worst_case_edge_basis_points: "10".to_string(),
        up_worst_case_edge_basis_points: Some("11".to_string()),
        down_worst_case_edge_basis_points: Some("9".to_string()),
        gate_blocked_by: Vec::new(),
        pricing_blocked_by: vec![BoltV3EntryPricingBlockReason::RealizedVolNotReady],
        fast_venue_name: Some("fast-source".to_string()),
        fast_venue_age_ms: Some(20),
        fast_venue_jitter_ms: Some(3),
        fast_venue_incoherent: false,
        lead_agreement_corr: Some("0.98".to_string()),
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
        clamp_outcome: None,
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
        execution_client_id: "execution-client-one".to_string(),
        client_order_id: snapshot.client_order_id.clone(),
        instrument_id: snapshot.submission_instrument_id.clone(),
        notional: "0.50".to_string(),
        economics_quote_id: "test-economics-quote".to_string(),
        economics_core_total: "-0.01".to_string(),
        economics_core_net_edge: "0.04".to_string(),
        economics_core_edge_ratio: "0.08".to_string(),
        economics_forecast_net_edge: "0.04".to_string(),
        economics_valid_until_ns: 1_300,
        economics_source_snapshot_ids: vec!["test-economics-snapshot".to_string()],
        intent_kind: BoltV3SubmitIntentKind::Entry,
        outcome: BoltV3AdmissionOutcome::Admitted,
        loss_halt_reasons: Vec::new(),
        snapshot_present: true,
        snapshot_observed_at_ns: Some(1_000),
        admission_now_ns: 1_200,
        snapshot_age_ns: Some(200),
        max_snapshot_age_ns: Some(1_000),
        snapshot_source: Some(BoltV3LossSnapshotSource::NtPortfolioSnapshot),
        per_trade_pnl_present: true,
        daily_pnl_present: true,
        rolling_pnl_present: true,
        current_equity_present: true,
        peak_equity_present: true,
        last_account_state_observed_at_ns: None,
        last_portfolio_snapshot_observed_at_ns: None,
        last_position_event_observed_at_ns: None,
        stale_reason: None,
        loss_snapshot_observed_at_ns: Some(1_000),
        loss_eval_now_ns: Some(1_200),
    };
    [
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
    ]
}

fn sample_submit_reservation_metadata() -> BoltV3SubmitReservationMetadataEvidence {
    BoltV3SubmitReservationMetadataEvidence {
        client_order_id: "client-order-one".to_string(),
        submit_reservation_id: "client-order-one#1".to_string(),
        venue_id: "POLYMARKET".to_string(),
        account_id: "POLYMARKET-001".to_string(),
        product_kind: "prediction_market_binary".to_string(),
        collateral_currency: "PUSD".to_string(),
        capital_pool_id: "polymarket-prediction-live".to_string(),
        collateral_group_id: "condition-one".to_string(),
        instrument_id: "condition-one-yes.POLYMARKET".to_string(),
        side: "buy".to_string(),
        submitted_quantity: "10".to_string(),
        liability_factor: "0.4".to_string(),
        additive_liability: "0.3".to_string(),
        reserved_liability: "4.3".to_string(),
        observed_at_ns: 1_000,
        source: "submit_admission".to_string(),
    }
}

fn sample_submit_reservation_fill() -> BoltV3SubmitReservationFillEvidence {
    BoltV3SubmitReservationFillEvidence {
        client_order_id: "client-order-one".to_string(),
        submit_reservation_id: "client-order-one#1".to_string(),
        trade_id: "trade-one".to_string(),
        instrument_id: "condition-one-yes.POLYMARKET".to_string(),
        side: "buy".to_string(),
        fill_quantity: "3".to_string(),
        observed_at_ns: 1_500,
        reconciliation: false,
        source: "submit_admission".to_string(),
    }
}

fn sample_basket_admission_decision_line() -> serde_json::Value {
    serde_json::json!({
        "schema_version": BOLT_V3_DECISION_EVIDENCE_SCHEMA_VERSION,
        "recorded_at_utc_ns": 2_i64,
        "gate_id": BOLT_V3_SUBMIT_ADMISSION_GATE_ID,
        "gate_version": BOLT_V3_DECISION_EVIDENCE_GATE_VERSION,
        "kind": "basket_admission_decision",
        "decision": BoltV3BasketAdmissionDecisionEvidence {
            strategy_id: "complete-set-arb".to_string(),
            execution_client_id: "polymarket-main".to_string(),
            basket_id: "basket-one".to_string(),
            group_id: "group-one".to_string(),
            leg_instrument_ids: vec![
                "condition-one-yes.POLYMARKET".to_string(),
                "condition-one-no.POLYMARKET".to_string(),
            ],
            total_notional: "1.0".to_string(),
            leg_order_count: 2,
            outcome: BoltV3BasketAdmissionOutcome::Admitted,
        },
    })
}

fn write_decision_evidence_lines(path: &std::path::Path, lines: &[serde_json::Value]) {
    let mut body = String::new();
    for line in lines {
        body.push_str(&serde_json::to_string(&line).expect("line should serialize"));
        body.push('\n');
    }
    std::fs::write(path, body).expect("decision evidence should write");
}

fn fixture_decision_evidence_line(relative_path: &str) -> serde_json::Value {
    let fixture = support::repo_text(relative_path);
    let mut lines = fixture.lines();
    let line = lines
        .next()
        .unwrap_or_else(|| panic!("fixture `{relative_path}` should contain one JSONL line"));
    assert!(
        lines.next().is_none(),
        "fixture `{relative_path}` should contain exactly one JSONL line"
    );
    serde_json::from_str(line)
        .unwrap_or_else(|error| panic!("fixture `{relative_path}` should parse: {error}"))
}

fn sample_exit_evaluation_evidence(populated: bool) -> BoltV3ExitEvaluationEvidence {
    if populated {
        BoltV3ExitEvaluationEvidence {
            position_id: Some("position-one".to_string()),
            market_id: Some("market-one".to_string()),
            instrument_id: Some("instrument-up".to_string()),
            client_order_id: Some("client-order-one".to_string()),
            exit_eval_now_ms: 1_700_000_000_000,
            exit_trigger_source: BoltV3ExitTriggerSource::ReferenceUpdate,
            trigger_ts_event_ms: Some(1_699_999_999_500),
            trigger_ts_init_ms: Some(1_699_999_999_800),
            rv_surface_id: "surface-one".to_string(),
            rv_as_of_ms: Some(1_699_999_995_000),
            rv_ready: true,
            rv_snapshot_receive_watermark_ms: Some(1_200),
            rv_max_source_age_ms: Some(500),
            rv_blockers: vec!["source_stale".to_string()],
            rv_source_diagnostics: vec!["source-a:ready".to_string()],
            rv_gate_result: BoltV3RvGateResult::RejectedFutureDated,
            rv_as_of_minus_now_ms: Some(-5_000),
            spot_price: Some("3100.5".to_string()),
            spot_venue_name: Some("venue-one".to_string()),
            fast_venue_available: true,
            reference_current_price: Some("3099.75".to_string()),
            reference_current_price_available: true,
            interval_open: Some("3100".to_string()),
            fair_probability_up: Some("0.55".to_string()),
            fair_probability_down: Some("0.45".to_string()),
            uncertainty_band_probability: Some("0.02".to_string()),
            hold_ev_bps: Some("12.5".to_string()),
            exit_ev_bps: Some("-3.0".to_string()),
            exit_decision: BoltV3ExitDecisionOutcome::ExitFailClosed,
            forced_flat_reasons: vec!["rv_gate_rejected".to_string()],
            submission_order_side: Some("Sell".to_string()),
            submission_price: Some("0.49".to_string()),
            submission_quantity: Some("1".to_string()),
            submission_blocked_reason: Some("rv_gate_rejected".to_string()),
        }
    } else {
        BoltV3ExitEvaluationEvidence {
            position_id: None,
            market_id: None,
            instrument_id: None,
            client_order_id: None,
            exit_eval_now_ms: 1_700_000_100_000,
            exit_trigger_source: BoltV3ExitTriggerSource::SignalQuote,
            trigger_ts_event_ms: None,
            trigger_ts_init_ms: None,
            rv_surface_id: "surface-two".to_string(),
            rv_as_of_ms: None,
            rv_ready: false,
            rv_snapshot_receive_watermark_ms: None,
            rv_max_source_age_ms: None,
            rv_blockers: Vec::new(),
            rv_source_diagnostics: Vec::new(),
            rv_gate_result: BoltV3RvGateResult::MissingSnapshot,
            rv_as_of_minus_now_ms: None,
            spot_price: None,
            spot_venue_name: None,
            fast_venue_available: false,
            reference_current_price: None,
            reference_current_price_available: false,
            interval_open: None,
            fair_probability_up: None,
            fair_probability_down: None,
            uncertainty_band_probability: None,
            hold_ev_bps: None,
            exit_ev_bps: None,
            exit_decision: BoltV3ExitDecisionOutcome::Hold,
            forced_flat_reasons: Vec::new(),
            submission_order_side: None,
            submission_price: None,
            submission_quantity: None,
            submission_blocked_reason: None,
        }
    }
}

fn sample_loss_governor_halt_evidence(populated: bool) -> BoltV3LossGovernorHaltEvidence {
    if populated {
        BoltV3LossGovernorHaltEvidence {
            snapshot_present: true,
            snapshot_observed_at_ns: Some(1_700_000_000_000_000_000),
            admission_now_ns: 1_700_000_005_000_000_000,
            snapshot_age_ns: Some(5_000_000_000),
            max_snapshot_age_ns: 5_000_000_000,
            snapshot_source: Some("portfolio_snapshot".to_string()),
            has_per_trade_pnl: true,
            has_daily_pnl: true,
            has_rolling_pnl: false,
            has_current_equity: true,
            has_peak_equity: false,
            last_account_state_ts_ns: Some(1_699_999_999_000_000_000),
            last_portfolio_snapshot_ts_ns: Some(1_700_000_000_000_000_000),
            last_position_event_ts_ns: Some(1_699_999_998_000_000_000),
            account_state_count: 3,
            portfolio_snapshot_count: 1,
            position_event_count: 7,
            stale_reason: BoltV3StaleLossReason::AgeExceeded,
            stable_halt_key: "halt-key-one".to_string(),
            retry_count: 2,
            elapsed_since_first_halt_ns: 10_000_000_000,
        }
    } else {
        BoltV3LossGovernorHaltEvidence {
            snapshot_present: false,
            snapshot_observed_at_ns: None,
            admission_now_ns: 1_700_000_010_000_000_000,
            snapshot_age_ns: None,
            max_snapshot_age_ns: 5_000_000_000,
            snapshot_source: None,
            has_per_trade_pnl: false,
            has_daily_pnl: false,
            has_rolling_pnl: false,
            has_current_equity: false,
            has_peak_equity: false,
            last_account_state_ts_ns: None,
            last_portfolio_snapshot_ts_ns: None,
            last_position_event_ts_ns: None,
            account_state_count: 0,
            portfolio_snapshot_count: 0,
            position_event_count: 0,
            stale_reason: BoltV3StaleLossReason::MissingSnapshot,
            stable_halt_key: "halt-key-two".to_string(),
            retry_count: 0,
            elapsed_since_first_halt_ns: 0,
        }
    }
}

fn sample_order_reject_evidence(populated: bool) -> BoltV3OrderRejectEvidence {
    if populated {
        BoltV3OrderRejectEvidence {
            reject_source: BoltV3RejectSource::Venue,
            reject_reason: BoltV3OrderRejectReason::MinNotionalRejected,
            admission_outcome: Some(BoltV3AdmissionOutcome::Admitted),
            raw_reason_text: Some("min notional not met".to_string()),
            instrument_id: "instrument-up".to_string(),
            order_side: Some("Buy".to_string()),
            raw_price: Some("0.50".to_string()),
            raw_quantity: Some("1".to_string()),
            raw_maker_amount: Some("0.50".to_string()),
            raw_taker_amount: Some("0.50".to_string()),
            normalized_price: Some("0.50".to_string()),
            normalized_quantity: Some("1".to_string()),
            normalized_maker_amount: Some("0.50".to_string()),
            normalized_taker_amount: Some("0.50".to_string()),
            venue_price_precision: Some(2),
            venue_size_precision: Some(0),
            venue_min_notional: Some("1.0".to_string()),
            prior_client_order_id: Some("client-order-zero".to_string()),
            client_order_id: "client-order-one".to_string(),
            retry_count: 1,
            backoff_cooldown_state: Some("cooling".to_string()),
            stable_episode_key: "episode-key-one".to_string(),
            elapsed_ns: 2_000_000_000,
        }
    } else {
        BoltV3OrderRejectEvidence {
            reject_source: BoltV3RejectSource::SubmitAdmission,
            reject_reason: BoltV3OrderRejectReason::AdmissionRejected,
            admission_outcome: None,
            raw_reason_text: None,
            instrument_id: "instrument-down".to_string(),
            order_side: None,
            raw_price: None,
            raw_quantity: None,
            raw_maker_amount: None,
            raw_taker_amount: None,
            normalized_price: None,
            normalized_quantity: None,
            normalized_maker_amount: None,
            normalized_taker_amount: None,
            venue_price_precision: None,
            venue_size_precision: None,
            venue_min_notional: None,
            prior_client_order_id: None,
            client_order_id: "client-order-two".to_string(),
            retry_count: 0,
            backoff_cooldown_state: None,
            stable_episode_key: "episode-key-two".to_string(),
            elapsed_ns: 0,
        }
    }
}

fn sample_settlement_evidence(settlement_key: &str) -> BoltV3SettlementEvidence {
    BoltV3SettlementEvidence {
        strategy_id: "BINARYORACLEEDGETAKER-001".to_string(),
        settlement_key: settlement_key.to_string(),
        market_id: "MKT-1".to_string(),
        position_id: "P-1".to_string(),
        instrument_id: "condition-MKT-1-UP.POLYMARKET".to_string(),
        product_id: "condition-MKT-1-UP".to_string(),
        outcome_side: BoltV3OutcomeSide::Up,
        entry_order_side: "Buy".to_string(),
        quantity: "10.00".to_string(),
        entry_price: "0.45".to_string(),
        family_key: "updown".to_string(),
        strike_price: "3100.0".to_string(),
        resolution_instrument_id: "RESOLUTION.SOURCE".to_string(),
        resolution_ts_event_ns: 1_300_000_000,
        reference_close_price: "3101.0".to_string(),
        payout_per_share: "1".to_string(),
        terminal_value: "10".to_string(),
        realized_pnl: "5.5".to_string(),
        settlement_currency: "USDC".to_string(),
    }
}

fn sample_settlement_booking_error(settlement_key: &str) -> BoltV3SettlementBookingErrorEvidence {
    BoltV3SettlementBookingErrorEvidence {
        strategy_id: "BINARYORACLEEDGETAKER-001".to_string(),
        settlement_key: settlement_key.to_string(),
        market_id: Some("MKT-1".to_string()),
        position_id: Some("P-1".to_string()),
        instrument_id: Some("condition-MKT-1-UP.POLYMARKET".to_string()),
        resolution_instrument_id: Some("RESOLUTION.SOURCE".to_string()),
        reason: BoltV3SettlementBookingErrorReason::ResolutionFeedMissing,
        detail: "resolution feed missing at market end; settlement not booked".to_string(),
        observed_at_ns: 1_300_000_000,
        terminal_lifecycle: None,
    }
}

fn sample_terminal_settlement_lifecycle() -> BoltV3OrderLifecycleEvidence {
    BoltV3OrderLifecycleEvidence {
        strategy_id: "BINARYORACLEEDGETAKER-001".to_string(),
        transition: BoltV3OrderLifecycleTransition::SettlementBookingTerminal,
        outcome: BoltV3OrderLifecycleOutcome::Flat,
        source: "settlement_booking_terminal".to_string(),
        market_id: Some("MKT-1".to_string()),
        instrument_id: Some("condition-MKT-1-UP.POLYMARKET".to_string()),
        position_id: Some("P-1".to_string()),
        client_order_id: None,
        prior_client_order_id: None,
        raw_reason_text: Some("settlement booking terminal".to_string()),
        order_side: Some("Buy".to_string()),
        filled_quantity: None,
        residual_quantity: Some("10.00".to_string()),
        ts_event_ns: Some(1_300_000_000),
    }
}

fn exit_evaluation_evidence_line(evidence: &BoltV3ExitEvaluationEvidence) -> serde_json::Value {
    serde_json::json!({
        "schema_version": BOLT_V3_DECISION_EVIDENCE_SCHEMA_VERSION,
        "recorded_at_utc_ns": 10_i64,
        "gate_id": BOLT_V3_EXIT_EVALUATION_GATE_ID,
        "gate_version": BOLT_V3_DECISION_EVIDENCE_GATE_VERSION,
        "kind": BOLT_V3_EXIT_EVALUATION_RECORD_KIND,
        "evidence": evidence,
    })
}

fn loss_governor_halt_evidence_line(
    evidence: &BoltV3LossGovernorHaltEvidence,
) -> serde_json::Value {
    serde_json::json!({
        "schema_version": BOLT_V3_DECISION_EVIDENCE_SCHEMA_VERSION,
        "recorded_at_utc_ns": 11_i64,
        "gate_id": BOLT_V3_LOSS_GOVERNOR_HALT_GATE_ID,
        "gate_version": BOLT_V3_DECISION_EVIDENCE_GATE_VERSION,
        "kind": BOLT_V3_LOSS_GOVERNOR_HALT_RECORD_KIND,
        "evidence": evidence,
    })
}

fn order_reject_evidence_line(evidence: &BoltV3OrderRejectEvidence) -> serde_json::Value {
    serde_json::json!({
        "schema_version": BOLT_V3_DECISION_EVIDENCE_SCHEMA_VERSION,
        "recorded_at_utc_ns": 12_i64,
        "gate_id": BOLT_V3_ORDER_REJECT_GATE_ID,
        "gate_version": BOLT_V3_DECISION_EVIDENCE_GATE_VERSION,
        "kind": BOLT_V3_ORDER_REJECT_RECORD_KIND,
        "evidence": evidence,
    })
}

fn settlement_evidence_line(evidence: &BoltV3SettlementEvidence) -> serde_json::Value {
    serde_json::json!({
        "schema_version": BOLT_V3_DECISION_EVIDENCE_SCHEMA_VERSION,
        "recorded_at_utc_ns": 13_i64,
        "gate_id": BOLT_V3_SETTLEMENT_GATE_ID,
        "gate_version": BOLT_V3_DECISION_EVIDENCE_GATE_VERSION,
        "kind": BOLT_V3_SETTLEMENT_RECORD_KIND,
        "settlement": evidence,
    })
}

fn settlement_booking_error_evidence_line(
    evidence: &BoltV3SettlementBookingErrorEvidence,
) -> serde_json::Value {
    serde_json::json!({
        "schema_version": BOLT_V3_DECISION_EVIDENCE_SCHEMA_VERSION,
        "recorded_at_utc_ns": 14_i64,
        "gate_id": BOLT_V3_SETTLEMENT_GATE_ID,
        "gate_version": BOLT_V3_DECISION_EVIDENCE_GATE_VERSION,
        "kind": BOLT_V3_SETTLEMENT_BOOKING_ERROR_RECORD_KIND,
        "booking_error": evidence,
    })
}

#[test]
fn exit_evaluation_evidence_round_trips_populated_and_sparse_records() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let evidence_path = temp.path().join("decision-evidence.jsonl");
    let records = vec![
        sample_exit_evaluation_evidence(true),
        sample_exit_evaluation_evidence(false),
    ];
    let lines: Vec<serde_json::Value> = records.iter().map(exit_evaluation_evidence_line).collect();
    write_decision_evidence_lines(&evidence_path, &lines);

    let read_back = read_exit_evaluation_evidence(&evidence_path, 100_000)
        .expect("exit-evaluation evidence should read back");

    assert_eq!(read_back, records);
}

#[test]
fn exit_evaluation_observed_inputs_default_absent_for_predeploy_lines() {
    let line = fixture_decision_evidence_line(
        "tests/fixtures/bolt_v3/predeploy_exit_evaluation_evidence.jsonl",
    );
    assert_eq!(line["kind"], BOLT_V3_EXIT_EVALUATION_RECORD_KIND);

    let decoded: BoltV3ExitEvaluationEvidence = serde_json::from_value(line["evidence"].clone())
        .expect("predeploy exit evaluation should decode");

    assert_eq!(decoded.spot_price, None);
    assert_eq!(decoded.spot_venue_name, None);
    assert!(!decoded.fast_venue_available);
    assert_eq!(decoded.reference_current_price, None);
    assert!(!decoded.reference_current_price_available);
    assert_eq!(decoded.interval_open, None);
    assert_eq!(decoded.fair_probability_up, None);
    assert_eq!(decoded.fair_probability_down, None);
    assert_eq!(decoded.uncertainty_band_probability, None);
}

#[test]
fn loss_governor_halt_evidence_round_trips_populated_and_sparse_records() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let evidence_path = temp.path().join("decision-evidence.jsonl");
    let records = vec![
        sample_loss_governor_halt_evidence(true),
        sample_loss_governor_halt_evidence(false),
    ];
    let lines: Vec<serde_json::Value> = records
        .iter()
        .map(loss_governor_halt_evidence_line)
        .collect();
    write_decision_evidence_lines(&evidence_path, &lines);

    let read_back = read_loss_governor_halt_evidence(&evidence_path, 100_000)
        .expect("loss-governor-halt evidence should read back");

    assert_eq!(read_back, records);
}

#[test]
fn order_reject_evidence_round_trips_populated_and_sparse_records() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let evidence_path = temp.path().join("decision-evidence.jsonl");
    let records = vec![
        sample_order_reject_evidence(true),
        sample_order_reject_evidence(false),
    ];
    let lines: Vec<serde_json::Value> = records.iter().map(order_reject_evidence_line).collect();
    write_decision_evidence_lines(&evidence_path, &lines);

    let read_back = read_order_reject_evidence(&evidence_path, 100_000)
        .expect("order-reject evidence should read back");

    assert_eq!(read_back, records);
}

#[test]
fn settlement_and_booking_error_evidence_round_trip_from_jsonl_writer() {
    let (_temp, evidence_path, writer) = temp_decision_evidence_writer("settlement-evidence");
    let settlement = sample_settlement_evidence("MKT-1:P-1");
    let booking_error = sample_settlement_booking_error("MKT-1:P-2");

    writer
        .record_settlement(&settlement)
        .expect("settlement evidence should write");
    writer
        .record_settlement_booking_error(&booking_error)
        .expect("settlement booking-error evidence should write");

    let lines = read_decision_evidence_json_lines(&evidence_path);
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0]["kind"], BOLT_V3_SETTLEMENT_RECORD_KIND);
    assert_eq!(lines[0]["gate_id"], BOLT_V3_SETTLEMENT_GATE_ID);
    assert_eq!(
        lines[1]["kind"],
        BOLT_V3_SETTLEMENT_BOOKING_ERROR_RECORD_KIND
    );
    assert_eq!(lines[1]["gate_id"], BOLT_V3_SETTLEMENT_GATE_ID);

    let settlements = read_settlement_evidence(&evidence_path, 100_000)
        .expect("settlement evidence should read back");
    let errors = read_settlement_booking_error_evidence(&evidence_path, 100_000)
        .expect("settlement booking-error evidence should read back");
    assert_eq!(settlements, vec![settlement]);
    assert_eq!(errors, vec![booking_error]);
}

#[test]
fn live_and_restart_terminal_settlement_share_one_canonical_schema() {
    let (_temp, evidence_path, writer) =
        temp_decision_evidence_writer("terminal-settlement-evidence");
    let booking_error = sample_settlement_booking_error("MKT-1:P-TERMINAL");
    let live = BoltV3TerminalSettlementEvidence {
        settlement_key: booking_error.settlement_key.clone(),
        booking_error: Some(booking_error),
        lifecycle: sample_terminal_settlement_lifecycle(),
    };
    let restart = BoltV3TerminalSettlementEvidence {
        settlement_key: "MKT-1:P-RESTART".to_string(),
        booking_error: None,
        lifecycle: sample_terminal_settlement_lifecycle(),
    };

    writer
        .record_terminal_settlement(&live)
        .expect("live terminal settlement evidence should write atomically");
    writer
        .record_terminal_settlement(&restart)
        .expect("restart terminal settlement evidence should use the same writer");

    let lines = read_decision_evidence_json_lines(&evidence_path);
    assert_eq!(lines.len(), 2);
    assert!(
        lines
            .iter()
            .all(|line| line["kind"] == BOLT_V3_TERMINAL_SETTLEMENT_RECORD_KIND)
    );
    assert_eq!(
        lines[0]["terminal_settlement"]["lifecycle"]["transition"],
        "settlement_booking_terminal"
    );
    assert_eq!(
        read_terminal_settlement_evidence(&evidence_path, 100_000)
            .expect("canonical terminal evidence should read back"),
        vec![live, restart]
    );
    assert_eq!(
        read_settlement_booking_error_keys_for_recovery_scope(
            &evidence_path,
            100_000,
            &BTreeSet::from(["MKT-1:P-TERMINAL".to_string()]),
        )
        .expect("replayed canonical terminal records should recover idempotently"),
        BTreeSet::from(["MKT-1:P-TERMINAL".to_string()])
    );
}

#[test]
fn legacy_nested_terminal_booking_error_remains_readable() {
    let (_temp, evidence_path, writer) = temp_decision_evidence_writer("legacy-terminal-evidence");
    let mut booking_error = sample_settlement_booking_error("MKT-1:P-LEGACY");
    booking_error.terminal_lifecycle = Some(sample_terminal_settlement_lifecycle());
    writer
        .record_settlement_booking_error(&booking_error)
        .expect("legacy terminal booking-error evidence should write");
    assert_eq!(
        read_settlement_booking_error_evidence(&evidence_path, 100_000)
            .expect("legacy nested terminal evidence should read back"),
        vec![booking_error]
    );
}

#[test]
fn settlement_recovery_reader_filters_by_structural_recovery_scope() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let evidence_path = temp.path().join("decision-evidence.jsonl");
    let in_scope = sample_settlement_evidence("MKT-1:P-RECOVERED");
    let out_of_scope = sample_settlement_evidence("MKT-2:P-OLD");
    write_decision_evidence_lines(
        &evidence_path,
        &[
            settlement_evidence_line(&out_of_scope),
            settlement_booking_error_evidence_line(&sample_settlement_booking_error(
                "MKT-1:P-RECOVERED",
            )),
            settlement_evidence_line(&in_scope),
        ],
    );

    let recovered = read_settlement_keys_for_recovery_scope(
        &evidence_path,
        100_000,
        &BTreeSet::from(["MKT-1:P-RECOVERED".to_string(), "MKT-1:P-OPEN".to_string()]),
    )
    .expect("settlement keys should be read with a structural position scope");

    assert_eq!(recovered, BTreeSet::from(["MKT-1:P-RECOVERED".to_string()]));
}

#[test]
fn settlement_booking_error_recovery_reader_filters_by_same_structural_scope() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let evidence_path = temp.path().join("decision-evidence.jsonl");
    let in_scope = sample_settlement_booking_error("MKT-1:P-RECOVERED");
    let out_of_scope = sample_settlement_booking_error("MKT-2:P-OLD");
    write_decision_evidence_lines(
        &evidence_path,
        &[
            settlement_booking_error_evidence_line(&out_of_scope),
            settlement_booking_error_evidence_line(&in_scope),
        ],
    );

    let recovered = read_settlement_booking_error_keys_for_recovery_scope(
        &evidence_path,
        100_000,
        &BTreeSet::from(["MKT-1:P-RECOVERED".to_string()]),
    )
    .expect("booking-error keys should be read with the same structural scope");

    assert_eq!(recovered, BTreeSet::from(["MKT-1:P-RECOVERED".to_string()]));
}

#[test]
fn settlement_evidence_recovery_reader_returns_in_scope_records_for_replay() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let evidence_path = temp.path().join("decision-evidence.jsonl");
    let in_scope = sample_settlement_evidence("MKT-1:P-RECOVERED");
    let out_of_scope = sample_settlement_evidence("MKT-2:P-OLD");
    write_decision_evidence_lines(
        &evidence_path,
        &[
            settlement_evidence_line(&out_of_scope),
            settlement_evidence_line(&in_scope),
        ],
    );

    let recovered = read_settlement_evidence_for_recovery_scope(
        &evidence_path,
        100_000,
        &BTreeSet::from(["MKT-1:P-RECOVERED".to_string()]),
    )
    .expect("settlement evidence should be replayable within the same structural scope");

    assert_eq!(recovered, vec![in_scope]);
}

#[test]
fn settlement_recovery_reader_ignores_duplicate_out_of_scope_keys() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let evidence_path = temp.path().join("decision-evidence.jsonl");
    let in_scope = sample_settlement_evidence("MKT-1:P-RECOVERED");
    let stale_duplicate = sample_settlement_evidence("MKT-2:P-OLD");
    write_decision_evidence_lines(
        &evidence_path,
        &[
            settlement_evidence_line(&stale_duplicate),
            settlement_evidence_line(&in_scope),
            settlement_evidence_line(&stale_duplicate),
        ],
    );

    let recovered = read_settlement_keys_for_recovery_scope(
        &evidence_path,
        100_000,
        &BTreeSet::from(["MKT-1:P-RECOVERED".to_string()]),
    )
    .expect("out-of-scope duplicates must not bound startup settlement recovery");

    assert_eq!(recovered, BTreeSet::from(["MKT-1:P-RECOVERED".to_string()]));
}

#[test]
fn settlement_recovery_reader_fails_closed_on_duplicate_in_scope_key() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let evidence_path = temp.path().join("decision-evidence.jsonl");
    let first = sample_settlement_evidence("MKT-1:P-DUP");
    let second = sample_settlement_evidence("MKT-1:P-DUP");
    write_decision_evidence_lines(
        &evidence_path,
        &[
            settlement_evidence_line(&first),
            settlement_evidence_line(&second),
        ],
    );

    let error = read_settlement_keys_for_recovery_scope(
        &evidence_path,
        100_000,
        &BTreeSet::from(["MKT-1:P-DUP".to_string()]),
    )
    .expect_err("duplicate settlement keys must fail closed");
    assert!(
        format!("{error:#}").contains("duplicate settlement key"),
        "duplicate settlement key error should be explicit: {error:#}"
    );
}

#[test]
fn entry_chain_and_recovery_readers_skip_new_rca_evidence_kinds() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let evidence_path = temp.path().join("decision-evidence.jsonl");
    let mut lines = sample_entry_decision_evidence_lines().to_vec();
    lines.push(serde_json::json!({
        "schema_version": BOLT_V3_DECISION_EVIDENCE_SCHEMA_VERSION,
        "recorded_at_utc_ns": 4_i64,
        "gate_id": BOLT_V3_SUBMIT_ADMISSION_GATE_ID,
        "gate_version": BOLT_V3_DECISION_EVIDENCE_GATE_VERSION,
        "kind": "submit_reservation_metadata",
        "metadata": sample_submit_reservation_metadata(),
    }));
    lines.push(exit_evaluation_evidence_line(
        &sample_exit_evaluation_evidence(true),
    ));
    lines.push(loss_governor_halt_evidence_line(
        &sample_loss_governor_halt_evidence(true),
    ));
    lines.push(order_reject_evidence_line(&sample_order_reject_evidence(
        true,
    )));
    write_decision_evidence_lines(&evidence_path, &lines);

    let chain = read_latest_entry_decision_evidence_chain(&evidence_path, 100_000)
        .expect("new RCA evidence kinds must not block entry-chain recovery");
    assert_eq!(chain.snapshot.client_order_id, "client-order-one");

    let recovery = read_submit_reservation_recovery_evidence(&evidence_path, 100_000)
        .expect("new RCA evidence kinds must not block submit-reservation recovery");
    assert!(
        recovery
            .metadata_by_client_order_id
            .contains_key("client-order-one"),
        "reservation metadata should recover alongside new RCA evidence kinds"
    );
}

#[test]
fn exit_evaluation_reader_returns_only_exit_evaluation_records() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let evidence_path = temp.path().join("decision-evidence.jsonl");
    let target = sample_exit_evaluation_evidence(true);
    let mut lines = sample_entry_decision_evidence_lines().to_vec();
    lines.push(loss_governor_halt_evidence_line(
        &sample_loss_governor_halt_evidence(true),
    ));
    lines.push(exit_evaluation_evidence_line(&target));
    lines.push(order_reject_evidence_line(&sample_order_reject_evidence(
        true,
    )));
    write_decision_evidence_lines(&evidence_path, &lines);

    let read_back = read_exit_evaluation_evidence(&evidence_path, 100_000)
        .expect("exit-evaluation reader should skip non-matching kinds");

    assert_eq!(read_back, vec![target]);
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

    fn record_basket_admission_decision(
        &self,
        _decision: &BoltV3BasketAdmissionDecisionEvidence,
    ) -> Result<()> {
        Ok(())
    }

    fn record_capital_admission_rebuild_audit(
        &self,
        _audit: &BoltV3CapitalAdmissionRebuildAuditEvidence,
    ) -> Result<()> {
        Ok(())
    }

    fn record_submit_reservation_metadata(
        &self,
        _metadata: &BoltV3SubmitReservationMetadataEvidence,
    ) -> Result<()> {
        Ok(())
    }

    fn record_submit_reservation_fill(
        &self,
        _fill: &BoltV3SubmitReservationFillEvidence,
    ) -> Result<()> {
        Ok(())
    }

    fn record_entry_skip(&self, _skip: &BoltV3EntrySkipEvidence) -> Result<()> {
        anyhow::bail!("decision evidence path noop writer received entry-skip evidence")
    }

    fn record_exit_decision(&self, _decision: &BoltV3ExitDecisionEvidence) -> Result<()> {
        anyhow::bail!("decision evidence path noop writer received exit-decision evidence")
    }

    fn record_exit_evaluation(&self, _evidence: &BoltV3ExitEvaluationEvidence) -> Result<()> {
        Ok(())
    }

    fn record_loss_governor_halt(&self, _evidence: &BoltV3LossGovernorHaltEvidence) -> Result<()> {
        Ok(())
    }

    fn record_order_reject(&self, _evidence: &BoltV3OrderRejectEvidence) -> Result<()> {
        Ok(())
    }

    fn record_requote_throttle(&self, _throttle: &BoltV3RequoteThrottleEvidence) -> Result<()> {
        anyhow::bail!("decision evidence path noop writer received requote-throttle evidence")
    }

    fn record_settlement(&self, _evidence: &BoltV3SettlementEvidence) -> Result<()> {
        Ok(())
    }

    fn record_settlement_booking_error(
        &self,
        _evidence: &BoltV3SettlementBookingErrorEvidence,
    ) -> Result<()> {
        Ok(())
    }

    fn drain_shutdown(&self) -> Result<()> {
        // Deliberate no-op: this path fixture never owns durable evidence.
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
fn binary_oracle_edge_taker_routes_evidence_admission_and_submit_through_shared_policy() {
    // Whole-module text via the A0 source-integrity owner (single canonical
    // order across digest + text). At A0 the single-file identity case
    // reproduces the prior `include_str!` text byte-for-byte, so the
    // intra-file `.find()` ordering below stays valid. The migrating split
    // slice (A3/A6/A7) discharges the forward order-sensitivity constraint.
    let strategy_source =
        support::module_source_text(bolt_v2::bolt_v3_source_integrity::STRATEGY_KEY);
    let strategy_source = strategy_source.as_str();
    let execution_source = include_str!("../src/bolt_v3_order_execution.rs");
    let evidence_index = execution_source
        .find("decision_evidence.record_order_intent(&intent)")
        .expect("shared execution policy must record decision evidence");
    let admission_index = execution_source
        .find("submit_admission.admit(&request)")
        .expect("shared execution policy must submit through admission");
    let submit_index = execution_source
        .find("sink.submit_order_via_nt(order, context)")
        .expect("shared execution policy must delegate to the NT mutation sink");

    assert!(
        evidence_index < admission_index && admission_index < submit_index,
        "decision evidence must be recorded before submit admission before NT submit"
    );
    let strategy_input_index = strategy_source
        .find(".record_strategy_input_snapshot(&strategy_input_snapshot)")
        .expect("entry strategy input snapshot must be recorded");
    let evidence_wrapper_call_after_strategy_input = strategy_source[strategy_input_index..]
        .find("self.submit_order_with_decision_evidence(")
        .expect("entry path must submit through evidence wrapper");
    assert!(
        evidence_wrapper_call_after_strategy_input > 0,
        "entry strategy input snapshot must be recorded before order-intent evidence wrapper"
    );
    // This intentionally scans the strategy source set, including in-file tests,
    // but excludes the shared NT mutation sink itself because the sink is the
    // approved policy boundary verified above.
    let strategy_source_without_execution_sink = strategy_source.replace(execution_source, "");
    assert_eq!(
        strategy_source_without_execution_sink
            .matches("self.submit_order(")
            .count(),
        0,
        "strategy code must not call NT submit directly"
    );
}

#[test]
fn binary_oracle_edge_taker_exit_submit_threads_managed_position_id_to_shared_policy() {
    let source = support::module_source_text(bolt_v2::bolt_v3_source_integrity::STRATEGY_KEY);
    let source = source.as_str();

    assert!(
        source.contains(
            "BoltV3SubmitContext::with_client_id_and_position_id(\n                client_id,\n                managed_position.position.position_id,\n            )"
        ),
        "exit submits must pass the managed PositionId into shared execution policy"
    );
}

#[test]
fn exit_fast_venue_availability_is_not_position_spot_coupled() {
    let exit_decision_source =
        support::repo_text("src/strategies/binary_oracle_edge_taker/exit_decision.rs");
    let strategy_source = support::repo_text("src/strategies/binary_oracle_edge_taker/mod.rs");

    assert!(
        !exit_decision_source.contains("fast_venue_available: fields.spot_price.is_some()"),
        "exit-decision evidence still derives fast_venue_available from position-coupled spot_price"
    );
    assert!(
        !strategy_source.contains("fast_venue_available: log_fields.spot_price.is_some()"),
        "exit-evaluation evidence still derives fast_venue_available from position-coupled spot_price"
    );
}

#[test]
fn exit_evaluation_optional_number_serialization_uses_finite_option_path() {
    let strategy_source = support::repo_text("src/strategies/binary_oracle_edge_taker/mod.rs");
    for field in [
        "spot_price",
        "reference_current_price",
        "interval_open",
        "fair_probability_up",
        "fair_probability_down",
        "uncertainty_band_probability",
        "hold_ev_bps",
        "exit_ev_bps",
        "submission_price",
    ] {
        let forbidden = format!("{field}: log_fields.{field}.map(evidence_number)");
        assert!(
            !strategy_source.contains(&forbidden),
            "exit-evaluation optional numeric field `{field}` still bypasses finite filtering"
        );
    }
}

#[test]
fn strategy_build_context_requires_decision_evidence_value() {
    let context = StrategyBuildContext::new(
        Arc::new(NoopDecisionEvidenceWriter),
        Arc::new(
            bolt_v2::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new(Arc::new(
                NoopDecisionEvidenceWriter,
            )),
        ),
        bolt_v2::bolt_v3_order_execution::BoltV3OrderExecutionPolicy::live(),
        support::fixture_execution_venue(),
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
                clamp_outcome: None,
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

#[derive(Clone, Copy)]
enum RvClockDomainReplayFamily {
    Decision,
    Evaluation,
}

fn rv_clock_domain_amendment_production_snapshot(
    as_of_ms: u64,
    watermark_ms: Option<u64>,
    raw_ready: bool,
    usable_ready: bool,
    has_blockers: bool,
) -> RealizedVolSnapshot {
    assert_eq!(
        usable_ready,
        raw_ready && !has_blockers,
        "fixture readiness must agree with the production usable-readiness contract"
    );
    let snapshot = RealizedVolSnapshot {
        surface_id: "<surface_id>".to_string(),
        as_of_ms,
        latest_accepted_receive_ms: watermark_ms.map(LocalReceiveMs::new),
        annualized_realized_vol_decimal: raw_ready.then_some(1.0),
        measured_annualized_realized_vol_decimal: raw_ready.then_some(1.0),
        noise_robust_annualized_realized_vol_decimal: raw_ready.then_some(1.0),
        continuous_annualized_realized_vol_decimal: raw_ready.then_some(1.0),
        jump_annualized_realized_vol_decimal: raw_ready.then_some(0.0),
        forecast_annualized_realized_vol_decimal: None,
        pricing_component: RealizedVolPricingComponent::Measured,
        ready: raw_ready,
        sources_used: raw_ready
            .then(|| "<SOURCE_ID_A>".to_string())
            .into_iter()
            .collect(),
        source_diagnostics: Vec::new(),
        horizon_estimates: Vec::new(),
        unknown_source_rejections: BTreeMap::new(),
        blocked_reasons: has_blockers
            .then_some(RealizedVolBlockReason::SourceStale)
            .into_iter()
            .collect(),
        aggregate_method: RealizedVolAggregation::UpperQuantile { quantile: 1.0 },
        seconds_per_annum: 31_536_000.0,
        config_fingerprint: "rv-clock-domain-replay".to_string(),
    };
    assert_eq!(
        snapshot.ready_realized_vol().is_some(),
        usable_ready,
        "fixture must exercise production readiness rather than a stored gate result"
    );
    snapshot
}

fn rv_clock_domain_amendment_wire_ms(
    record: &serde_json::Value,
    field: &str,
    family: RvClockDomainReplayFamily,
) -> Option<i128> {
    match family {
        RvClockDomainReplayFamily::Decision => record
            .get(field)
            .and_then(serde_json::Value::as_u64)
            .map(i128::from),
        RvClockDomainReplayFamily::Evaluation => record
            .get(field)
            .and_then(serde_json::Value::as_i64)
            .map(i128::from),
    }
}

fn rv_clock_domain_amendment_replayed_gate(
    record: &serde_json::Value,
    family: RvClockDomainReplayFamily,
) -> Option<BoltV3RvGateResult> {
    let max_source_age_ms = record
        .get("rv_max_source_age_ms")?
        .as_u64()
        .filter(|age| *age > 0)?;
    let usable_ready = match family {
        RvClockDomainReplayFamily::Decision => record
            .get("rv_snapshot_has_ready_realized_vol")?
            .as_bool()?,
        RvClockDomainReplayFamily::Evaluation => record.get("rv_ready")?.as_bool()?,
    };
    let snapshot_as_of_key = match family {
        RvClockDomainReplayFamily::Decision => "rv_snapshot_as_of_ms",
        RvClockDomainReplayFamily::Evaluation => "rv_as_of_ms",
    };
    if rv_clock_domain_amendment_wire_ms(record, snapshot_as_of_key, family).is_none() {
        return Some(BoltV3RvGateResult::MissingSnapshot);
    }

    let Some(evaluation_receive_ms) =
        rv_clock_domain_amendment_wire_ms(record, "trigger_ts_init_ms", family)
    else {
        return Some(BoltV3RvGateResult::MissingEvaluationEventTime);
    };
    if evaluation_receive_ms < 0 {
        return None;
    }
    let Some(snapshot_receive_watermark_ms) =
        rv_clock_domain_amendment_wire_ms(record, "rv_snapshot_receive_watermark_ms", family)
    else {
        return Some(BoltV3RvGateResult::RejectedNotReady);
    };
    if snapshot_receive_watermark_ms < 0 {
        return None;
    }
    if snapshot_receive_watermark_ms > evaluation_receive_ms {
        return Some(BoltV3RvGateResult::RejectedFutureDated);
    }
    if evaluation_receive_ms - snapshot_receive_watermark_ms > i128::from(max_source_age_ms) {
        return Some(BoltV3RvGateResult::RejectedStale);
    }
    if !usable_ready {
        return Some(BoltV3RvGateResult::RejectedNotReady);
    }
    Some(BoltV3RvGateResult::Accepted)
}

fn rv_clock_domain_amendment_round_trip_decision_value(
    value: serde_json::Value,
) -> serde_json::Value {
    let record: BoltV3ExitDecisionEvidence =
        serde_json::from_value(value).expect("decision payload should decode");
    serde_json::to_value(record).expect("decision payload should encode")
}

fn rv_clock_domain_amendment_round_trip_evaluation_value(
    value: serde_json::Value,
) -> serde_json::Value {
    let record: BoltV3ExitEvaluationEvidence =
        serde_json::from_value(value).expect("evaluation payload should decode");
    serde_json::to_value(record).expect("evaluation payload should encode")
}

#[test]
fn rv_clock_domain_amendment_exit_wires_preserve_new_and_legacy_inputs() {
    assert_eq!(BOLT_V3_DECISION_EVIDENCE_SCHEMA_VERSION, 15);

    let mut decision = serde_json::to_value(sample_exit_decision_evidence())
        .expect("sample exit decision should serialize");
    decision["rv_snapshot_as_of_ms"] = serde_json::json!(1_200_u64);
    decision["trigger_ts_init_ms"] = serde_json::json!(1_200_u64);
    let decision = rv_clock_domain_amendment_round_trip_decision_value(decision);
    assert_eq!(
        decision.get("rv_snapshot_receive_watermark_ms"),
        Some(&serde_json::json!(1_200_u64)),
        "decision wire must retain the receive-domain snapshot watermark"
    );
    assert_eq!(
        decision.get("rv_max_source_age_ms"),
        Some(&serde_json::json!(500_u64)),
        "decision wire must retain the effective configured age"
    );
    assert_eq!(
        decision.get("rv_snapshot_has_ready_realized_vol"),
        Some(&serde_json::json!(true)),
        "decision wire must retain readiness independently from the stored gate-filtered RV"
    );

    let mut evaluation = serde_json::to_value(sample_exit_evaluation_evidence(true))
        .expect("sample exit evaluation should serialize");
    evaluation["rv_as_of_ms"] = serde_json::json!(1_200_i64);
    evaluation["trigger_ts_init_ms"] = serde_json::json!(1_200_i64);
    let evaluation = rv_clock_domain_amendment_round_trip_evaluation_value(evaluation);
    assert_eq!(
        evaluation.get("rv_snapshot_receive_watermark_ms"),
        Some(&serde_json::json!(1_200_i64)),
        "evaluation wire must retain the checked signed watermark"
    );
    assert_eq!(
        evaluation.get("rv_max_source_age_ms"),
        Some(&serde_json::json!(500_u64)),
        "evaluation wire must retain the effective configured age"
    );

    let mut missing_snapshot = serde_json::to_value(sample_exit_decision_evidence())
        .expect("sample exit decision should serialize");
    missing_snapshot["rv_snapshot_as_of_ms"] = serde_json::Value::Null;
    missing_snapshot["rv_snapshot_receive_watermark_ms"] = serde_json::Value::Null;
    missing_snapshot["rv_max_source_age_ms"] = serde_json::json!(500_u64);
    missing_snapshot["rv_snapshot_has_ready_realized_vol"] = serde_json::json!(false);
    let missing_snapshot = rv_clock_domain_amendment_round_trip_decision_value(missing_snapshot);
    assert_eq!(
        missing_snapshot.get("rv_snapshot_has_ready_realized_vol"),
        Some(&serde_json::json!(false)),
        "new missing-snapshot decisions must write an explicit false replay input"
    );
    assert_eq!(
        rv_clock_domain_amendment_replayed_gate(
            &missing_snapshot,
            RvClockDomainReplayFamily::Decision,
        ),
        Some(BoltV3RvGateResult::MissingSnapshot)
    );

    for fixture in [
        "tests/fixtures/bolt_v3/predeploy_exit_decision_evidence.jsonl",
        "tests/fixtures/bolt_v3/predeploy_exit_evaluation_evidence.jsonl",
    ] {
        let legacy = fixture_decision_evidence_line(fixture);
        let payload = legacy
            .get("exit_decision")
            .or_else(|| legacy.get("evidence"))
            .expect("legacy fixture should carry an exit payload");
        assert!(payload.get("rv_max_source_age_ms").is_none());
        assert!(payload.get("rv_snapshot_receive_watermark_ms").is_none());
    }

    let mut null_markers = serde_json::to_value(sample_exit_decision_evidence())
        .expect("sample exit decision should serialize");
    null_markers["rv_max_source_age_ms"] = serde_json::Value::Null;
    null_markers["rv_snapshot_has_ready_realized_vol"] = serde_json::Value::Null;
    assert_eq!(
        rv_clock_domain_amendment_replayed_gate(&null_markers, RvClockDomainReplayFamily::Decision,),
        None,
        "omitted or null replay markers identify legacy evidence"
    );

    let mut zero_is_present = serde_json::to_value(sample_exit_evaluation_evidence(false))
        .expect("sample exit evaluation should serialize");
    zero_is_present["rv_max_source_age_ms"] = serde_json::json!(500_u64);
    zero_is_present["rv_as_of_ms"] = serde_json::json!(0_i64);
    zero_is_present["trigger_ts_init_ms"] = serde_json::Value::Null;
    zero_is_present["rv_snapshot_receive_watermark_ms"] = serde_json::json!(0_i64);
    assert_eq!(
        rv_clock_domain_amendment_replayed_gate(
            &zero_is_present,
            RvClockDomainReplayFamily::Evaluation,
        ),
        Some(BoltV3RvGateResult::MissingEvaluationEventTime),
        "an as-of value of zero is a present snapshot and must reach later precedence"
    );
}

#[test]
fn rv_clock_domain_amendment_negative_receive_fields_fail_decode_and_encode() {
    assert_eq!(BOLT_V3_DECISION_EVIDENCE_SCHEMA_VERSION, 15);

    for field in ["trigger_ts_init_ms", "rv_snapshot_receive_watermark_ms"] {
        let mut payload = serde_json::to_value(sample_exit_evaluation_evidence(true))
            .expect("sample exit evaluation should serialize");
        payload[field] = serde_json::json!(-1_i64);
        let error = serde_json::from_value::<BoltV3ExitEvaluationEvidence>(payload)
            .expect_err("negative receive-domain fields must fail direct payload decoding");
        assert!(
            error.to_string().contains(field),
            "decode error must name `{field}` exactly: {error}"
        );

        let mut line = exit_evaluation_evidence_line(&sample_exit_evaluation_evidence(true));
        line["evidence"][field] = serde_json::json!(-1_i64);
        let temp = tempfile::tempdir().expect("tempdir should create");
        let path = temp.path().join(format!("negative-{field}.jsonl"));
        write_decision_evidence_lines(&path, &[line]);
        let error = read_exit_evaluation_evidence(&path, 100_000)
            .expect_err("negative receive-domain fields must fail full-line decoding");
        assert!(
            format!("{error:#}").contains(field),
            "full-line decode error must name `{field}` exactly: {error:#}"
        );
    }

    let mut negative_trigger_init = sample_exit_evaluation_evidence(true);
    negative_trigger_init.trigger_ts_init_ms = Some(-1);
    let (_temp, _path, writer) = temp_decision_evidence_writer("negative-exit-evaluation-encode");
    let error = writer
        .record_exit_evaluation(&negative_trigger_init)
        .expect_err("manual negative trigger init must fail durable encoding");
    assert!(
        format!("{error:#}").contains("trigger_ts_init_ms"),
        "encode error must name trigger_ts_init_ms: {error:#}"
    );

    let mut negative_watermark = sample_exit_evaluation_evidence(true);
    negative_watermark.rv_snapshot_receive_watermark_ms = Some(-1);
    let (_temp, _path, writer) =
        temp_decision_evidence_writer("negative-exit-evaluation-watermark-encode");
    let error = writer
        .record_exit_evaluation(&negative_watermark)
        .expect_err("manual negative RV receive watermark must fail durable encoding");
    assert!(
        format!("{error:#}").contains("rv_snapshot_receive_watermark_ms"),
        "encode error must name rv_snapshot_receive_watermark_ms: {error:#}"
    );

    for (label, marker, expected_decoded, expected_encoded) in [
        ("omitted", None, None, serde_json::Value::Null),
        (
            "null",
            Some(serde_json::Value::Null),
            None,
            serde_json::Value::Null,
        ),
        (
            "zero",
            Some(serde_json::json!(0_i64)),
            Some(0_i64),
            serde_json::json!(0_i64),
        ),
        (
            "i64_max",
            Some(serde_json::json!(i64::MAX)),
            Some(i64::MAX),
            serde_json::json!(i64::MAX),
        ),
    ] {
        let mut payload = serde_json::to_value(sample_exit_evaluation_evidence(true))
            .expect("sample exit evaluation should serialize");
        if let Some(marker) = marker {
            payload["rv_snapshot_receive_watermark_ms"] = marker;
        } else {
            payload
                .as_object_mut()
                .expect("evaluation payload should be an object")
                .remove("rv_snapshot_receive_watermark_ms");
            assert!(
                payload.get("rv_snapshot_receive_watermark_ms").is_none(),
                "omission coverage must decode a payload with no watermark key"
            );
        }
        let decoded: BoltV3ExitEvaluationEvidence = serde_json::from_value(payload)
            .expect("omitted/null/non-negative watermark must decode");
        assert_eq!(
            decoded.rv_snapshot_receive_watermark_ms, expected_decoded,
            "{label} watermark must decode to its exact Option value"
        );
        let encoded = serde_json::to_value(decoded).expect("valid watermark must encode");
        assert_eq!(
            encoded.get("rv_snapshot_receive_watermark_ms"),
            Some(&expected_encoded),
            "{label} watermark must round-trip to its canonical exact value: {encoded}"
        );
    }

    let mut defensive = serde_json::to_value(sample_exit_evaluation_evidence(true))
        .expect("sample exit evaluation should serialize");
    defensive["rv_max_source_age_ms"] = serde_json::json!(500_u64);
    defensive["rv_snapshot_receive_watermark_ms"] = serde_json::json!(-1_i64);
    assert_eq!(
        rv_clock_domain_amendment_replayed_gate(&defensive, RvClockDomainReplayFamily::Evaluation,),
        None,
        "defensive replay must reject negative receive-domain inputs"
    );
}

#[test]
fn rv_clock_domain_amendment_records_recompute_gate_from_owned_inputs() {
    #[derive(Clone, Copy)]
    struct Case {
        label: &'static str,
        as_of_ms: Option<i64>,
        evaluation_receive_ms: Option<i64>,
        watermark_ms: Option<i64>,
        raw_ready: bool,
        usable_ready: bool,
        blockers: &'static [&'static str],
        expected: BoltV3RvGateResult,
    }

    let cases = [
        Case {
            label: "missing_snapshot_and_evaluation",
            as_of_ms: None,
            evaluation_receive_ms: None,
            watermark_ms: None,
            raw_ready: false,
            usable_ready: false,
            blockers: &[],
            expected: BoltV3RvGateResult::MissingSnapshot,
        },
        Case {
            label: "missing_snapshot",
            as_of_ms: None,
            evaluation_receive_ms: Some(1_200),
            watermark_ms: Some(1_200),
            raw_ready: false,
            usable_ready: false,
            blockers: &[],
            expected: BoltV3RvGateResult::MissingSnapshot,
        },
        Case {
            label: "missing_evaluation",
            as_of_ms: Some(1_200),
            evaluation_receive_ms: None,
            watermark_ms: Some(1_200),
            raw_ready: true,
            usable_ready: true,
            blockers: &[],
            expected: BoltV3RvGateResult::MissingEvaluationEventTime,
        },
        Case {
            label: "missing_evaluation_and_watermark",
            as_of_ms: Some(1_200),
            evaluation_receive_ms: None,
            watermark_ms: None,
            raw_ready: true,
            usable_ready: true,
            blockers: &[],
            expected: BoltV3RvGateResult::MissingEvaluationEventTime,
        },
        Case {
            label: "missing_watermark",
            as_of_ms: Some(1_200),
            evaluation_receive_ms: Some(1_200),
            watermark_ms: None,
            raw_ready: true,
            usable_ready: true,
            blockers: &[],
            expected: BoltV3RvGateResult::RejectedNotReady,
        },
        Case {
            label: "not_ready_plus_future",
            as_of_ms: Some(1_200),
            evaluation_receive_ms: Some(1_200),
            watermark_ms: Some(1_201),
            raw_ready: false,
            usable_ready: false,
            blockers: &[],
            expected: BoltV3RvGateResult::RejectedFutureDated,
        },
        Case {
            label: "not_ready_plus_stale",
            as_of_ms: Some(1_200),
            evaluation_receive_ms: Some(1_701),
            watermark_ms: Some(1_200),
            raw_ready: false,
            usable_ready: false,
            blockers: &[],
            expected: BoltV3RvGateResult::RejectedStale,
        },
        Case {
            label: "blocker_plus_stale",
            as_of_ms: Some(1_200),
            evaluation_receive_ms: Some(1_701),
            watermark_ms: Some(1_200),
            raw_ready: true,
            usable_ready: false,
            blockers: &["source_stale"],
            expected: BoltV3RvGateResult::RejectedStale,
        },
        Case {
            label: "not_ready",
            as_of_ms: Some(1_200),
            evaluation_receive_ms: Some(1_200),
            watermark_ms: Some(1_200),
            raw_ready: false,
            usable_ready: false,
            blockers: &[],
            expected: BoltV3RvGateResult::RejectedNotReady,
        },
        Case {
            label: "accepted_zero",
            as_of_ms: Some(0),
            evaluation_receive_ms: Some(0),
            watermark_ms: Some(0),
            raw_ready: true,
            usable_ready: true,
            blockers: &[],
            expected: BoltV3RvGateResult::Accepted,
        },
    ];

    for case in cases {
        let production_snapshot = case.as_of_ms.map(|as_of_ms| {
            rv_clock_domain_amendment_production_snapshot(
                u64::try_from(as_of_ms).expect("fixture as-of time must be non-negative"),
                case.watermark_ms.map(|watermark_ms| {
                    u64::try_from(watermark_ms)
                        .expect("fixture receive watermark must be non-negative")
                }),
                case.raw_ready,
                case.usable_ready,
                !case.blockers.is_empty(),
            )
        });
        if let Some(snapshot) = production_snapshot.as_ref() {
            assert_eq!(snapshot.ready, case.raw_ready);
            assert_eq!(snapshot.ready_realized_vol().is_some(), case.usable_ready);
            assert_eq!(
                snapshot
                    .latest_accepted_receive_ms
                    .map(LocalReceiveMs::value),
                case.watermark_ms.map(|watermark_ms| {
                    u64::try_from(watermark_ms)
                        .expect("fixture receive watermark must be non-negative")
                })
            );
            assert_eq!(
                snapshot.blocked_reasons.is_empty(),
                case.blockers.is_empty()
            );
        }
        let production_result = classify_rv_gate(
            production_snapshot.as_ref(),
            case.evaluation_receive_ms.map(|evaluation_receive_ms| {
                LocalReceiveMs::new(
                    u64::try_from(evaluation_receive_ms)
                        .expect("fixture evaluation receive time must be non-negative"),
                )
            }),
            Some(500),
        );
        assert_eq!(
            production_result, case.expected,
            "{} must retain production classifier precedence",
            case.label
        );
        let poisoned_stored_gate = if case.expected == BoltV3RvGateResult::Accepted {
            "rejected_stale"
        } else {
            "accepted"
        };

        for family in [
            RvClockDomainReplayFamily::Decision,
            RvClockDomainReplayFamily::Evaluation,
        ] {
            let mut value = match family {
                RvClockDomainReplayFamily::Decision => {
                    serde_json::to_value(sample_exit_decision_evidence())
                        .expect("sample exit decision should serialize")
                }
                RvClockDomainReplayFamily::Evaluation => {
                    serde_json::to_value(sample_exit_evaluation_evidence(true))
                        .expect("sample exit evaluation should serialize")
                }
            };
            value["rv_max_source_age_ms"] = serde_json::json!(500_u64);
            value["trigger_ts_init_ms"] = case
                .evaluation_receive_ms
                .map_or(serde_json::Value::Null, |value| serde_json::json!(value));
            value["rv_snapshot_receive_watermark_ms"] = case
                .watermark_ms
                .map_or(serde_json::Value::Null, |value| serde_json::json!(value));
            value["rv_gate_result"] = serde_json::json!(poisoned_stored_gate);
            match family {
                RvClockDomainReplayFamily::Decision => {
                    value["rv_snapshot_as_of_ms"] = case
                        .as_of_ms
                        .map_or(serde_json::Value::Null, |value| serde_json::json!(value));
                    value["rv_snapshot_ready"] = serde_json::json!(case.raw_ready);
                    value["rv_snapshot_has_ready_realized_vol"] =
                        serde_json::json!(case.usable_ready);
                    value["rv_snapshot_blockers"] = serde_json::json!(case.blockers);
                    value["realized_vol"] = if case.usable_ready {
                        serde_json::json!("999")
                    } else {
                        serde_json::Value::Null
                    };
                    value = rv_clock_domain_amendment_round_trip_decision_value(value);
                }
                RvClockDomainReplayFamily::Evaluation => {
                    value["rv_as_of_ms"] = case
                        .as_of_ms
                        .map_or(serde_json::Value::Null, |value| serde_json::json!(value));
                    value["rv_ready"] = serde_json::json!(case.usable_ready);
                    value["rv_blockers"] = serde_json::json!(case.blockers);
                    value = rv_clock_domain_amendment_round_trip_evaluation_value(value);
                }
            }
            assert_eq!(
                value.get("rv_gate_result"),
                Some(&serde_json::json!(poisoned_stored_gate)),
                "{} must retain the deliberately wrong stored gate",
                case.label
            );
            let blockers_field = match family {
                RvClockDomainReplayFamily::Decision => "rv_snapshot_blockers",
                RvClockDomainReplayFamily::Evaluation => "rv_blockers",
            };
            assert_eq!(
                value.get(blockers_field),
                Some(&serde_json::json!(case.blockers)),
                "{} must retain its explicit blocker state",
                case.label
            );
            match family {
                RvClockDomainReplayFamily::Decision => {
                    assert_eq!(
                        value.get("rv_snapshot_has_ready_realized_vol"),
                        Some(&serde_json::json!(case.usable_ready)),
                        "{} must retain usable decision readiness",
                        case.label
                    );
                    assert_eq!(
                        value.get("rv_snapshot_ready"),
                        Some(&serde_json::json!(case.raw_ready)),
                        "{} must retain raw snapshot readiness",
                        case.label
                    );
                }
                RvClockDomainReplayFamily::Evaluation => {
                    assert_eq!(
                        value.get("rv_ready"),
                        Some(&serde_json::json!(case.usable_ready)),
                        "{} must retain usable evaluation readiness",
                        case.label
                    );
                }
            }
            assert_eq!(
                rv_clock_domain_amendment_replayed_gate(&value, family),
                Some(production_result),
                "{} must replay the public production classifier from record-local inputs only",
                case.label
            );
        }
    }

    let high_watermark_ms =
        u64::try_from(i64::MAX).expect("i64::MAX should fit the decision u64 wire") + 1;
    let high_trigger_ms = high_watermark_ms + 501;
    let mut high_decision = serde_json::to_value(sample_exit_decision_evidence())
        .expect("sample exit decision should serialize");
    high_decision["rv_snapshot_as_of_ms"] = serde_json::json!(high_watermark_ms);
    high_decision["trigger_ts_init_ms"] = serde_json::json!(high_trigger_ms);
    high_decision["rv_snapshot_receive_watermark_ms"] = serde_json::json!(high_watermark_ms);
    high_decision["rv_max_source_age_ms"] = serde_json::json!(500_u64);
    high_decision["rv_snapshot_has_ready_realized_vol"] = serde_json::json!(true);
    high_decision["rv_snapshot_ready"] = serde_json::json!(true);
    high_decision["rv_snapshot_blockers"] = serde_json::json!([]);
    high_decision["rv_gate_result"] = serde_json::json!("accepted");
    let high_decision = rv_clock_domain_amendment_round_trip_decision_value(high_decision);
    for (field, expected) in [
        ("rv_snapshot_as_of_ms", high_watermark_ms),
        ("trigger_ts_init_ms", high_trigger_ms),
        ("rv_snapshot_receive_watermark_ms", high_watermark_ms),
    ] {
        assert_eq!(
            high_decision.get(field).and_then(serde_json::Value::as_u64),
            Some(expected),
            "high-u64 decision timestamp `{field}` must remain present on its unsigned wire"
        );
    }
    let high_snapshot = rv_clock_domain_amendment_production_snapshot(
        high_watermark_ms,
        Some(high_watermark_ms),
        true,
        true,
        false,
    );
    let high_production_result = classify_rv_gate(
        Some(&high_snapshot),
        Some(LocalReceiveMs::new(high_trigger_ms)),
        Some(500),
    );
    assert_eq!(
        high_production_result,
        BoltV3RvGateResult::RejectedStale,
        "production must compare unsigned receive timestamps above i64::MAX"
    );
    assert_eq!(
        rv_clock_domain_amendment_replayed_gate(
            &high_decision,
            RvClockDomainReplayFamily::Decision,
        ),
        Some(high_production_result),
        "decision replay must compare unsigned timestamps above i64::MAX through i128"
    );

    for family in [
        RvClockDomainReplayFamily::Decision,
        RvClockDomainReplayFamily::Evaluation,
    ] {
        let mut legacy = match family {
            RvClockDomainReplayFamily::Decision => {
                serde_json::to_value(sample_exit_decision_evidence()).unwrap()
            }
            RvClockDomainReplayFamily::Evaluation => {
                serde_json::to_value(sample_exit_evaluation_evidence(true)).unwrap()
            }
        };
        legacy
            .as_object_mut()
            .expect("record should be an object")
            .remove("rv_max_source_age_ms");
        assert_eq!(
            rv_clock_domain_amendment_replayed_gate(&legacy, family),
            None,
            "missing marker must remain legacy-unreplayable"
        );
        legacy["rv_max_source_age_ms"] = serde_json::json!(0_u64);
        assert_eq!(
            rv_clock_domain_amendment_replayed_gate(&legacy, family),
            None,
            "non-positive marker must remain legacy-unreplayable"
        );
    }
}
