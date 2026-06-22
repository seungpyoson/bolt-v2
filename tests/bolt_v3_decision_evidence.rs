mod support;

use std::{collections::BTreeMap, sync::Arc};

use anyhow::Result;
use bolt_v2::{
    bolt_v3_config::load_bolt_v3_config,
    bolt_v3_decision_evidence::{
        BOLT_V3_DECISION_EVIDENCE_GATE_VERSION, BOLT_V3_DECISION_EVIDENCE_SCHEMA_VERSION,
        BOLT_V3_ENTRY_SKIP_GATE_ID, BOLT_V3_EXIT_DECISION_GATE_ID,
        BOLT_V3_LOSS_GOVERNOR_HALT_GATE_ID, BOLT_V3_ORDER_INTENT_GATE_ID,
        BOLT_V3_REQUOTE_THROTTLE_GATE_ID, BOLT_V3_STRATEGY_INPUT_SNAPSHOT_GATE_ID,
        BOLT_V3_SUBMIT_ADMISSION_GATE_ID, BoltV3AdmissionDecisionEvidence, BoltV3AdmissionOutcome,
        BoltV3BasketAdmissionDecisionEvidence, BoltV3BasketAdmissionOutcome,
        BoltV3DecisionEvidenceWriter, BoltV3EntryBlockReason, BoltV3EntryPricingBlockReason,
        BoltV3EntrySkipEvidence, BoltV3EntrySkipReasonCategory, BoltV3ExitDecisionEvidence,
        BoltV3ExitDecisionOutcome, BoltV3ExitRvGateResult, BoltV3ExitRvSnapshotBlocker,
        BoltV3ExitTriggerSource, BoltV3ForcedFlatReason, BoltV3LossGovernorHaltEvidence,
        BoltV3LossHaltReason, BoltV3LossSnapshotSource, BoltV3LossSnapshotStaleReason,
        BoltV3OrderIntentEvidence, BoltV3OrderIntentKind, BoltV3OrderIntentOrderFields,
        BoltV3OutcomeSide, BoltV3PositionSizerRebuildAuditEvidence,
        BoltV3RealizedVolatilitySourceDiagnosticEvidence, BoltV3RequoteActionCostClass,
        BoltV3RequoteThrottleBlockReason, BoltV3RequoteThrottleBound,
        BoltV3RequoteThrottleEvidence, BoltV3StrategyInputEvidenceSnapshot, BoltV3SubmitIntentKind,
        BoltV3SubmitReservationFillEvidence, BoltV3SubmitReservationMetadataEvidence,
        BoltV3TradingState, JsonlBoltV3DecisionEvidenceWriter, decision_evidence_path,
        read_latest_entry_decision_evidence_chain, read_submit_reservation_recovery_evidence,
    },
    bolt_v3_realized_volatility::{
        RealizedVolBlockReason, RealizedVolSampleKind, RealizedVolSourceClass,
        RealizedVolSourceDiagnostic, RealizedVolSourceRejectReason, RealizedVolSourceStatus,
    },
    strategies::registry::FeeProvider,
    strategies::registry::StrategyBuildContext,
};
use futures_util::future::{BoxFuture, FutureExt};
use nautilus_model::enums::{OrderSide, OrderType, TimeInForce};
use nautilus_model::identifiers::InstrumentId;
use rust_decimal::Decimal;

struct NoopFeeProvider;

const EXPECTED_POSITION_SIZER_RECOVERY_SCHEMA_VERSION: u32 = 12;
const PRE_POSITION_SIZER_RECOVERY_SCHEMA_VERSION: u32 = 9;

impl FeeProvider for NoopFeeProvider {
    fn fee_bps(&self, _instrument_id: InstrumentId) -> Option<Decimal> {
        None
    }

    fn warm(&self, _instrument_id: InstrumentId) -> BoxFuture<'_, Result<()>> {
        async { Ok(()) }.boxed()
    }
}

#[test]
fn decision_evidence_schema_version_tracks_reference_price_and_position_sizer_records() {
    assert_eq!(
        BOLT_V3_DECISION_EVIDENCE_SCHEMA_VERSION,
        EXPECTED_POSITION_SIZER_RECOVERY_SCHEMA_VERSION
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
        reference_current_price: Some("3100.5".to_string()),
        reference_current_price_source_id: Some("chainlink_primary".to_string()),
        reference_current_price_failed_over: Some(false),
        realized_volatility: "2.5".to_string(),
        realized_volatility_surface_id: "<surface_id>".to_string(),
        realized_volatility_as_of_ms: Some(1200),
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
        fee_rate_basis_points: "0".to_string(),
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
        line["schema_version"] = serde_json::json!(PRE_POSITION_SIZER_RECOVERY_SCHEMA_VERSION);
    }
    lines.push(serde_json::json!({
        "schema_version": EXPECTED_POSITION_SIZER_RECOVERY_SCHEMA_VERSION,
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
                "schema_version": EXPECTED_POSITION_SIZER_RECOVERY_SCHEMA_VERSION,
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
            "gate_id": BOLT_V3_LOSS_GOVERNOR_HALT_GATE_ID,
            "gate_version": BOLT_V3_DECISION_EVIDENCE_GATE_VERSION,
            "kind": "loss_governor_halt",
            "loss_governor_halt": sample_loss_governor_halt_evidence(),
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
    assert_eq!(lines[0]["schema_version"], 12);
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
fn exit_decision_evidence_writes_one_durable_line_and_readers_skip_it() {
    let (_temp, evidence_path, writer) = temp_decision_evidence_writer("exit-decision");
    let evidence = sample_exit_decision_evidence();

    writer
        .record_exit_decision(&evidence)
        .expect("exit decision evidence should write through the durable writer");

    let lines = read_decision_evidence_json_lines(&evidence_path);
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0]["schema_version"], 12);
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

    append_decision_evidence_lines(&evidence_path, &sample_entry_decision_evidence_lines());
    read_latest_entry_decision_evidence_chain(&evidence_path, 100_000)
        .expect("exit decision record must not block entry-chain recovery");
    read_submit_reservation_recovery_evidence(&evidence_path, 100_000)
        .expect("exit decision record must not block submit-reservation recovery");
}

#[test]
fn loss_governor_halt_evidence_writes_one_durable_line_and_readers_skip_it() {
    let (_temp, evidence_path, writer) = temp_decision_evidence_writer("loss-governor-halt");
    let evidence = sample_loss_governor_halt_evidence();

    writer
        .record_loss_governor_halt(&evidence)
        .expect("loss governor halt evidence should write through the durable writer");

    let lines = read_decision_evidence_json_lines(&evidence_path);
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0]["schema_version"], 12);
    assert_eq!(lines[0]["kind"], "loss_governor_halt");
    let decoded: BoltV3LossGovernorHaltEvidence =
        serde_json::from_value(lines[0]["loss_governor_halt"].clone())
            .expect("loss governor halt should decode");
    assert_eq!(decoded, evidence);
    assert_eq!(
        decoded.halt_reasons,
        vec![BoltV3LossHaltReason::DailyLossLimit]
    );
    assert_eq!(decoded.max_snapshot_age_ns, 1_000);
    assert_eq!(
        decoded.snapshot_source,
        Some(BoltV3LossSnapshotSource::NtPortfolioSnapshot)
    );
    assert!(decoded.per_trade_pnl_present);
    assert!(decoded.daily_pnl_present);
    assert!(decoded.rolling_pnl_present);
    assert!(decoded.current_equity_present);
    assert!(decoded.peak_equity_present);
    assert_eq!(decoded.last_account_state_observed_at_ns, Some(9_400));
    assert_eq!(decoded.last_portfolio_snapshot_observed_at_ns, Some(9_500));
    assert_eq!(decoded.last_position_event_observed_at_ns, Some(9_450));
    assert_eq!(decoded.target_trading_state, BoltV3TradingState::Reducing);

    append_decision_evidence_lines(&evidence_path, &sample_entry_decision_evidence_lines());
    read_latest_entry_decision_evidence_chain(&evidence_path, 100_000)
        .expect("loss governor halt record must not block entry-chain recovery");
    read_submit_reservation_recovery_evidence(&evidence_path, 100_000)
        .expect("loss governor halt record must not block submit-reservation recovery");
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
    assert_eq!(lines[0]["schema_version"], 12);
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
            "schema_version": PRE_POSITION_SIZER_RECOVERY_SCHEMA_VERSION,
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
        realized_vol: Some("2.5".to_string()),
        realized_vol_source_venue: Some("fast-source".to_string()),
        realized_vol_source_ts_ms: Some(1_100),
        fair_probability_up: Some("0.6".to_string()),
        fair_probability_down: Some("0.4".to_string()),
        selected_side: Some(BoltV3OutcomeSide::Up),
        sized_notional: Some("25".to_string()),
        sized_worst_case_ev_bps: Some("12.5".to_string()),
        sized_edge_cents_per_share: Some("1.25".to_string()),
        theta_scaled_min_edge_bps: Some("10".to_string()),
        up_fee_bps: Some("2".to_string()),
        down_fee_bps: Some("3".to_string()),
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

fn sample_exit_decision_evidence() -> BoltV3ExitDecisionEvidence {
    BoltV3ExitDecisionEvidence {
        strategy_id: "strategy-one".to_string(),
        market_id: Some("market-one".to_string()),
        position_id: Some("position-one".to_string()),
        position_instrument_id: Some("instrument-up".to_string()),
        position_outcome_side: Some(BoltV3OutcomeSide::Up),
        forced_flat_reasons: vec![BoltV3ForcedFlatReason::StaleReference],
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
        rv_snapshot_blockers: vec![BoltV3ExitRvSnapshotBlocker::QuorumNotReady],
        rv_source_diagnostics: Vec::new(),
        rv_gate_result: BoltV3ExitRvGateResult::RejectedFutureDated,
        rv_future_dating_delta_ms: Some(50),
        exit_hysteresis_bps: "1".to_string(),
        exit_decision: BoltV3ExitDecisionOutcome::ExitFailClosed,
        blocked_reason: None,
        client_order_id: Some("client-order-exit".to_string()),
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

fn sample_loss_governor_halt_evidence() -> BoltV3LossGovernorHaltEvidence {
    BoltV3LossGovernorHaltEvidence {
        observed_at_ns: 10_000,
        source: "nt_portfolio_snapshot".to_string(),
        halt_reasons: vec![BoltV3LossHaltReason::DailyLossLimit],
        snapshot_observed_at_ns: Some(9_500),
        admission_now_ns: 10_000,
        snapshot_age_ns: Some(500),
        snapshot_source: Some(BoltV3LossSnapshotSource::NtPortfolioSnapshot),
        per_trade_pnl_present: true,
        daily_pnl_present: true,
        rolling_pnl_present: true,
        current_equity_present: true,
        peak_equity_present: true,
        last_account_state_observed_at_ns: Some(9_400),
        last_portfolio_snapshot_observed_at_ns: Some(9_500),
        last_position_event_observed_at_ns: Some(9_450),
        per_trade_pnl: Some("-1".to_string()),
        daily_pnl: Some("-20".to_string()),
        rolling_pnl: Some("-20".to_string()),
        current_equity: Some("980".to_string()),
        peak_equity: Some("1000".to_string()),
        max_per_trade_loss: Some("10".to_string()),
        max_daily_loss: Some("20".to_string()),
        max_rolling_loss: Some("30".to_string()),
        max_drawdown: Some("40".to_string()),
        max_snapshot_age_ns: 1_000,
        stale_reason: Some(BoltV3LossSnapshotStaleReason::AgeExceeded),
        previous_trading_state: BoltV3TradingState::Active,
        target_trading_state: BoltV3TradingState::Reducing,
        subsystem: "loss_governor".to_string(),
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
        reference_current_price: Some("3100.5".to_string()),
        reference_current_price_source_id: Some("chainlink_primary".to_string()),
        reference_current_price_failed_over: Some(false),
        realized_volatility: "1.5".to_string(),
        realized_volatility_surface_id: String::new(),
        realized_volatility_as_of_ms: None,
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
        execution_client_id: "execution-client-one".to_string(),
        client_order_id: snapshot.client_order_id.clone(),
        instrument_id: snapshot.submission_instrument_id.clone(),
        notional: "0.50".to_string(),
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

    fn record_position_sizer_rebuild_audit(
        &self,
        _audit: &BoltV3PositionSizerRebuildAuditEvidence,
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

    fn record_loss_governor_halt(&self, _halt: &BoltV3LossGovernorHaltEvidence) -> Result<()> {
        anyhow::bail!("decision evidence path noop writer received loss-governor-halt evidence")
    }

    fn record_requote_throttle(&self, _throttle: &BoltV3RequoteThrottleEvidence) -> Result<()> {
        anyhow::bail!("decision evidence path noop writer received requote-throttle evidence")
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
        .find(".record_order_intent(&routing.intent)")
        .expect("shared execution policy must record decision evidence");
    let admission_index = execution_source
        .find("routing.submit_admission.admit(&routing.request)")
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
        .find("self.submit_order_with_decision_evidence(\n                    intent,\n                    order,\n                    BoltV3SubmitContext::with_client_id(client_id),\n                )")
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
fn strategy_build_context_requires_decision_evidence_value() {
    let context = StrategyBuildContext::new(
        Arc::new(NoopFeeProvider),
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
