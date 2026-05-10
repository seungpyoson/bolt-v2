use bolt_v2::bolt_v3_config::{CatalogFsProtocol, PersistenceBlock, RotationKind, StreamingBlock};
use bolt_v2::bolt_v3_decision_events::{
    BOLT_V3_ENTRY_EVALUATION_DECISION_EVENT_TYPE,
    BOLT_V3_ENTRY_ORDER_SUBMISSION_DECISION_EVENT_TYPE,
    BOLT_V3_ENTRY_PRE_SUBMIT_REJECTION_DECISION_EVENT_TYPE,
    BOLT_V3_EXIT_EVALUATION_DECISION_EVENT_TYPE, BOLT_V3_EXIT_ORDER_SUBMISSION_DECISION_EVENT_TYPE,
    BOLT_V3_EXIT_PRE_SUBMIT_REJECTION_DECISION_EVENT_TYPE,
    BOLT_V3_MARKET_SELECTION_DECISION_EVENT_TYPE, BOLT_V3_MARKET_SELECTION_FAILURE_REASONS,
    BoltV3DecisionEventCatalogHandoff, BoltV3DecisionEventCommonFields,
    BoltV3EntryEvaluationDecisionEvent, BoltV3EntryEvaluationFacts,
    BoltV3EntryOrderSubmissionDecisionEvent, BoltV3EntryPreSubmitRejectionDecisionEvent,
    BoltV3ExitEvaluationDecisionEvent, BoltV3ExitEvaluationFacts,
    BoltV3ExitOrderSubmissionDecisionEvent, BoltV3ExitPreSubmitRejectionDecisionEvent,
    BoltV3MarketSelectionDecisionEvent, BoltV3MarketSelectionResultFacts,
    BoltV3OrderSubmissionFacts, BoltV3PreSubmitRejectionFacts, BoltV3RejectedOrderFacts,
    register_bolt_v3_decision_event_types,
};
use nautilus_core::UnixNanos;
use nautilus_model::data::Data;
use nautilus_persistence::backend::catalog::ParquetDataCatalog;
use serde_json::{Value, json};
use std::fs;
use tempfile::TempDir;

#[test]
fn market_selection_result_event_writes_through_nt_catalog_handoff() {
    register_bolt_v3_decision_event_types();

    let temp_dir = TempDir::new().unwrap();
    let mut handoff = BoltV3DecisionEventCatalogHandoff::from_persistence_block(
        &persistence_block(temp_dir.path()),
    )
    .unwrap();

    let event = BoltV3MarketSelectionDecisionEvent::market_selection_result(
        common_fields(),
        BoltV3MarketSelectionResultFacts {
            market_selection_type: "rotating_market".to_string(),
            market_selection_timestamp_milliseconds: 1_000,
            market_selection_outcome: "current".to_string(),
            market_selection_failure_reason: None,
            rotating_market_family: Some("updown".to_string()),
            underlying_asset: Some("ETH".to_string()),
            cadence_seconds: Some(300),
            market_selection_rule: Some("active_or_next".to_string()),
            retry_interval_seconds: Some(5),
            blocked_after_seconds: Some(60),
            polymarket_condition_id: Some("condition-eth".to_string()),
            polymarket_market_slug: Some("eth-updown-5m-1000".to_string()),
            polymarket_question_id: Some("question-eth".to_string()),
            up_instrument_id: Some("condition-eth-UP.POLYMARKET".to_string()),
            down_instrument_id: Some("condition-eth-DOWN.POLYMARKET".to_string()),
            selected_market_observed_timestamp: Some(1_000),
            polymarket_market_start_timestamp_milliseconds: Some(1_000),
            polymarket_market_end_timestamp_milliseconds: Some(301_000),
            price_to_beat_value: Some(3_100.0),
            price_to_beat_observed_timestamp: Some(995),
            price_to_beat_source: Some("polymarket_gamma_market_anchor".to_string()),
        },
        UnixNanos::from(2_000),
        UnixNanos::from(2_001),
    )
    .unwrap();

    handoff.write_market_selection_result(event).unwrap();

    let ids = vec!["target-eth-updown".to_string()];
    let loaded = ParquetDataCatalog::new(temp_dir.path(), None, None, None, None)
        .query_custom_data_dynamic(
            BOLT_V3_MARKET_SELECTION_DECISION_EVENT_TYPE,
            Some(&ids),
            None,
            None,
            None,
            None,
            true,
        )
        .unwrap();

    assert_eq!(loaded.len(), 1);
    match &loaded[0] {
        Data::Custom(custom) => {
            assert_eq!(
                custom.data_type.type_name(),
                BOLT_V3_MARKET_SELECTION_DECISION_EVENT_TYPE
            );
            let decoded = custom
                .data
                .as_any()
                .downcast_ref::<bolt_v2::bolt_v3_decision_events::BoltV3MarketSelectionDecisionEvent>()
                .expect("BoltV3MarketSelectionDecisionEvent");
            assert_eq!(decoded.configured_target_id, "target-eth-updown");
            assert_eq!(
                decoded.event_facts.get("market_selection_failure_reason"),
                Some(&Value::Null)
            );
        }
        other => panic!("expected Data::Custom, got {other:?}"),
    }
}

#[test]
fn market_selection_result_rejects_missing_failure_reason_for_failed_outcome() {
    let error = BoltV3MarketSelectionDecisionEvent::market_selection_result(
        common_fields(),
        failed_market_selection_result_facts(None),
        UnixNanos::from(2_000),
        UnixNanos::from(2_001),
    )
    .unwrap_err();

    assert!(
        error
            .to_string()
            .contains("market_selection_failure_reason must be non-null")
    );
}

#[test]
fn market_selection_result_rejects_unknown_failure_reason() {
    let error = BoltV3MarketSelectionDecisionEvent::market_selection_result(
        common_fields(),
        failed_market_selection_result_facts(Some("some_new_reason")),
        UnixNanos::from(2_000),
        UnixNanos::from(2_001),
    )
    .unwrap_err();

    assert!(
        error
            .to_string()
            .contains("unsupported market_selection_failure_reason `some_new_reason`")
    );
}

#[test]
fn market_selection_result_accepts_allowed_failure_reasons() {
    for reason in BOLT_V3_MARKET_SELECTION_FAILURE_REASONS {
        BoltV3MarketSelectionDecisionEvent::market_selection_result(
            common_fields(),
            failed_market_selection_result_facts(Some(reason)),
            UnixNanos::from(2_000),
            UnixNanos::from(2_001),
        )
        .unwrap();
    }
}

#[test]
fn market_selection_result_handoff_returns_catalog_write_error() {
    let temp_dir = TempDir::new().unwrap();
    let file_path = temp_dir.path().join("not-a-directory");
    fs::write(&file_path, b"occupied").unwrap();

    let mut handoff =
        BoltV3DecisionEventCatalogHandoff::from_persistence_block(&persistence_block(&file_path))
            .unwrap();
    let event = BoltV3MarketSelectionDecisionEvent::market_selection_result(
        common_fields(),
        BoltV3MarketSelectionResultFacts {
            market_selection_type: "rotating_market".to_string(),
            market_selection_timestamp_milliseconds: 1_000,
            market_selection_outcome: "current".to_string(),
            market_selection_failure_reason: None,
            rotating_market_family: Some("updown".to_string()),
            underlying_asset: Some("ETH".to_string()),
            cadence_seconds: Some(300),
            market_selection_rule: Some("active_or_next".to_string()),
            retry_interval_seconds: Some(5),
            blocked_after_seconds: Some(60),
            polymarket_condition_id: Some("condition-eth".to_string()),
            polymarket_market_slug: Some("eth-updown-5m-1000".to_string()),
            polymarket_question_id: Some("question-eth".to_string()),
            up_instrument_id: Some("condition-eth-UP.POLYMARKET".to_string()),
            down_instrument_id: Some("condition-eth-DOWN.POLYMARKET".to_string()),
            selected_market_observed_timestamp: Some(1_000),
            polymarket_market_start_timestamp_milliseconds: Some(1_000),
            polymarket_market_end_timestamp_milliseconds: Some(301_000),
            price_to_beat_value: Some(3_100.0),
            price_to_beat_observed_timestamp: Some(995),
            price_to_beat_source: Some("polymarket_gamma_market_anchor".to_string()),
        },
        UnixNanos::from(2_000),
        UnixNanos::from(2_001),
    )
    .unwrap();

    let error = handoff.write_market_selection_result(event).unwrap_err();

    assert!(!error.to_string().is_empty());
}

#[test]
fn entry_evaluation_event_writes_through_nt_catalog_handoff() {
    let temp_dir = TempDir::new().unwrap();
    let mut handoff = BoltV3DecisionEventCatalogHandoff::from_persistence_block(
        &persistence_block(temp_dir.path()),
    )
    .unwrap();

    let event = BoltV3EntryEvaluationDecisionEvent::entry_evaluation(
        common_fields(),
        entry_evaluation_facts(),
        UnixNanos::from(2_500),
        UnixNanos::from(2_501),
    )
    .unwrap();

    handoff.write_entry_evaluation(event).unwrap();

    let loaded = query_events(
        temp_dir.path(),
        BOLT_V3_ENTRY_EVALUATION_DECISION_EVENT_TYPE,
    );

    assert_eq!(loaded.len(), 1);
    match &loaded[0] {
        Data::Custom(custom) => {
            let decoded = custom
                .data
                .as_any()
                .downcast_ref::<bolt_v2::bolt_v3_decision_events::BoltV3EntryEvaluationDecisionEvent>()
                .expect("BoltV3EntryEvaluationDecisionEvent");
            assert_eq!(decoded.decision_event_type, "entry_evaluation");
            assert_eq!(
                decoded.event_facts.get("entry_no_action_reason"),
                Some(&Value::Null)
            );
            assert_eq!(
                decoded.event_facts.get("archetype_metrics"),
                Some(&json!({"expected_edge_basis_points": 42.0}))
            );
        }
        other => panic!("expected Data::Custom, got {other:?}"),
    }
}

#[test]
fn entry_evaluation_rejects_no_action_without_reason() {
    let mut facts = entry_evaluation_facts();
    facts.entry_decision = "no_action".to_string();
    facts.updown_side = None;
    facts.entry_no_action_reason = None;

    let error = BoltV3EntryEvaluationDecisionEvent::entry_evaluation(
        common_fields(),
        facts,
        UnixNanos::from(2_500),
        UnixNanos::from(2_501),
    )
    .unwrap_err();

    assert!(
        error
            .to_string()
            .contains("entry_no_action_reason must be non-null")
    );
}

#[test]
fn entry_order_submission_event_writes_through_nt_catalog_handoff() {
    let temp_dir = TempDir::new().unwrap();
    let mut handoff = BoltV3DecisionEventCatalogHandoff::from_persistence_block(
        &persistence_block(temp_dir.path()),
    )
    .unwrap();

    let event = BoltV3EntryOrderSubmissionDecisionEvent::entry_order_submission(
        common_fields(),
        order_submission_facts(Some("ORDER-001".to_string())),
        UnixNanos::from(3_000),
        UnixNanos::from(3_001),
    )
    .unwrap();

    handoff.write_entry_order_submission(event).unwrap();

    let ids = vec!["target-eth-updown".to_string()];
    let loaded = ParquetDataCatalog::new(temp_dir.path(), None, None, None, None)
        .query_custom_data_dynamic(
            BOLT_V3_ENTRY_ORDER_SUBMISSION_DECISION_EVENT_TYPE,
            Some(&ids),
            None,
            None,
            None,
            None,
            true,
        )
        .unwrap();

    assert_eq!(loaded.len(), 1);
    match &loaded[0] {
        Data::Custom(custom) => {
            let decoded = custom
                .data
                .as_any()
                .downcast_ref::<bolt_v2::bolt_v3_decision_events::BoltV3EntryOrderSubmissionDecisionEvent>()
                .expect("BoltV3EntryOrderSubmissionDecisionEvent");
            assert_eq!(
                decoded.event_facts.get("client_order_id"),
                Some(&Value::String("ORDER-001".to_string()))
            );
        }
        other => panic!("expected Data::Custom, got {other:?}"),
    }
}

#[test]
fn entry_order_submission_rejects_missing_client_order_id() {
    let error = BoltV3EntryOrderSubmissionDecisionEvent::entry_order_submission(
        common_fields(),
        order_submission_facts(None),
        UnixNanos::from(3_000),
        UnixNanos::from(3_001),
    )
    .unwrap_err();

    assert!(
        error
            .to_string()
            .contains("client_order_id must be non-null")
    );
}

#[test]
fn entry_pre_submit_rejection_event_writes_null_client_order_id() {
    let temp_dir = TempDir::new().unwrap();
    let mut handoff = BoltV3DecisionEventCatalogHandoff::from_persistence_block(
        &persistence_block(temp_dir.path()),
    )
    .unwrap();

    let event = BoltV3EntryPreSubmitRejectionDecisionEvent::entry_pre_submit_rejection(
        common_fields(),
        BoltV3PreSubmitRejectionFacts {
            order: BoltV3RejectedOrderFacts::from(order_submission_facts(None)),
            rejection_reason: "invalid_quantity".to_string(),
            authoritative_position_quantity: None,
            authoritative_sellable_quantity: None,
            open_exit_order_quantity: None,
            uncovered_position_quantity: None,
        },
        UnixNanos::from(4_000),
        UnixNanos::from(4_001),
    )
    .unwrap();

    handoff.write_entry_pre_submit_rejection(event).unwrap();

    let ids = vec!["target-eth-updown".to_string()];
    let loaded = ParquetDataCatalog::new(temp_dir.path(), None, None, None, None)
        .query_custom_data_dynamic(
            BOLT_V3_ENTRY_PRE_SUBMIT_REJECTION_DECISION_EVENT_TYPE,
            Some(&ids),
            None,
            None,
            None,
            None,
            true,
        )
        .unwrap();

    assert_eq!(loaded.len(), 1);
    match &loaded[0] {
        Data::Custom(custom) => {
            let decoded = custom
                .data
                .as_any()
                .downcast_ref::<bolt_v2::bolt_v3_decision_events::BoltV3EntryPreSubmitRejectionDecisionEvent>()
                .expect("BoltV3EntryPreSubmitRejectionDecisionEvent");
            assert_eq!(
                decoded.event_facts.get("client_order_id"),
                Some(&Value::Null)
            );
            assert_eq!(
                decoded.event_facts.get("entry_pre_submit_rejection_reason"),
                Some(&Value::String("invalid_quantity".to_string()))
            );
        }
        other => panic!("expected Data::Custom, got {other:?}"),
    }
}

#[test]
fn exit_order_submission_event_writes_through_nt_catalog_handoff() {
    let temp_dir = TempDir::new().unwrap();
    let mut handoff = BoltV3DecisionEventCatalogHandoff::from_persistence_block(
        &persistence_block(temp_dir.path()),
    )
    .unwrap();

    let event = BoltV3ExitOrderSubmissionDecisionEvent::exit_order_submission(
        common_fields(),
        order_submission_facts(Some("EXIT-ORDER-001".to_string())),
        UnixNanos::from(5_000),
        UnixNanos::from(5_001),
    )
    .unwrap();

    handoff.write_exit_order_submission(event).unwrap();

    let loaded = query_events(
        temp_dir.path(),
        BOLT_V3_EXIT_ORDER_SUBMISSION_DECISION_EVENT_TYPE,
    );

    assert_eq!(loaded.len(), 1);
    match &loaded[0] {
        Data::Custom(custom) => {
            let decoded = custom
                .data
                .as_any()
                .downcast_ref::<bolt_v2::bolt_v3_decision_events::BoltV3ExitOrderSubmissionDecisionEvent>()
                .expect("BoltV3ExitOrderSubmissionDecisionEvent");
            assert_eq!(
                decoded.event_facts.get("client_order_id"),
                Some(&Value::String("EXIT-ORDER-001".to_string()))
            );
        }
        other => panic!("expected Data::Custom, got {other:?}"),
    }
}

#[test]
fn exit_pre_submit_rejection_event_writes_null_client_order_id() {
    let temp_dir = TempDir::new().unwrap();
    let mut handoff = BoltV3DecisionEventCatalogHandoff::from_persistence_block(
        &persistence_block(temp_dir.path()),
    )
    .unwrap();

    let event = BoltV3ExitPreSubmitRejectionDecisionEvent::exit_pre_submit_rejection(
        common_fields(),
        BoltV3PreSubmitRejectionFacts {
            order: BoltV3RejectedOrderFacts::from(order_submission_facts(None)),
            rejection_reason: "invalid_quantity".to_string(),
            authoritative_position_quantity: Some(10.0),
            authoritative_sellable_quantity: Some(10.0),
            open_exit_order_quantity: Some(0.0),
            uncovered_position_quantity: Some(10.0),
        },
        UnixNanos::from(6_000),
        UnixNanos::from(6_001),
    )
    .unwrap();

    handoff.write_exit_pre_submit_rejection(event).unwrap();

    let loaded = query_events(
        temp_dir.path(),
        BOLT_V3_EXIT_PRE_SUBMIT_REJECTION_DECISION_EVENT_TYPE,
    );

    assert_eq!(loaded.len(), 1);
    match &loaded[0] {
        Data::Custom(custom) => {
            let decoded = custom
                .data
                .as_any()
                .downcast_ref::<bolt_v2::bolt_v3_decision_events::BoltV3ExitPreSubmitRejectionDecisionEvent>()
                .expect("BoltV3ExitPreSubmitRejectionDecisionEvent");
            assert_eq!(
                decoded.event_facts.get("client_order_id"),
                Some(&Value::Null)
            );
            assert_eq!(
                decoded.event_facts.get("exit_pre_submit_rejection_reason"),
                Some(&Value::String("invalid_quantity".to_string()))
            );
        }
        other => panic!("expected Data::Custom, got {other:?}"),
    }
}

#[test]
fn exit_evaluation_event_writes_through_nt_catalog_handoff() {
    let temp_dir = TempDir::new().unwrap();
    let mut handoff = BoltV3DecisionEventCatalogHandoff::from_persistence_block(
        &persistence_block(temp_dir.path()),
    )
    .unwrap();

    let event = BoltV3ExitEvaluationDecisionEvent::exit_evaluation(
        common_fields(),
        exit_evaluation_facts(),
        UnixNanos::from(6_500),
        UnixNanos::from(6_501),
    )
    .unwrap();

    handoff.write_exit_evaluation(event).unwrap();

    let loaded = query_events(temp_dir.path(), BOLT_V3_EXIT_EVALUATION_DECISION_EVENT_TYPE);

    assert_eq!(loaded.len(), 1);
    match &loaded[0] {
        Data::Custom(custom) => {
            let decoded = custom
                .data
                .as_any()
                .downcast_ref::<bolt_v2::bolt_v3_decision_events::BoltV3ExitEvaluationDecisionEvent>()
                .expect("BoltV3ExitEvaluationDecisionEvent");
            assert_eq!(decoded.decision_event_type, "exit_evaluation");
            assert_eq!(
                decoded
                    .event_facts
                    .get("exit_order_mechanical_rejection_reason"),
                Some(&Value::Null)
            );
            assert_eq!(
                decoded.event_facts.get("archetype_metrics"),
                Some(&json!({}))
            );
        }
        other => panic!("expected Data::Custom, got {other:?}"),
    }
}

#[test]
fn exit_evaluation_rejects_missing_mechanical_rejection_reason() {
    let mut facts = exit_evaluation_facts();
    facts.exit_order_mechanical_outcome = "rejected".to_string();
    facts.exit_order_mechanical_rejection_reason = None;
    facts.exit_decision = "hold".to_string();
    facts.exit_decision_reason = "exit_order_mechanical_rejection".to_string();

    let error = BoltV3ExitEvaluationDecisionEvent::exit_evaluation(
        common_fields(),
        facts,
        UnixNanos::from(6_500),
        UnixNanos::from(6_501),
    )
    .unwrap_err();

    assert!(
        error
            .to_string()
            .contains("exit_order_mechanical_rejection_reason must be non-null")
    );
}

fn common_fields() -> BoltV3DecisionEventCommonFields {
    BoltV3DecisionEventCommonFields {
        schema_version: 1,
        decision_trace_id: "123e4567-e89b-12d3-a456-426614174002".to_string(),
        strategy_instance_id: "strategy-alpha".to_string(),
        strategy_archetype: "edge_taker".to_string(),
        trader_id: "TRADER-001".to_string(),
        client_id: "POLY-A".to_string(),
        venue: "POLYMARKET".to_string(),
        runtime_mode: "live".to_string(),
        release_id: "release-sha".to_string(),
        config_hash: "config-hash".to_string(),
        nautilus_trader_revision: "38b912a8b0fe14e4046773973ff46a3b798b1e3e".to_string(),
        configured_target_id: "target-eth-updown".to_string(),
    }
}

fn failed_market_selection_result_facts(reason: Option<&str>) -> BoltV3MarketSelectionResultFacts {
    BoltV3MarketSelectionResultFacts {
        market_selection_type: "rotating_market".to_string(),
        market_selection_timestamp_milliseconds: 1_000,
        market_selection_outcome: "failed".to_string(),
        market_selection_failure_reason: reason.map(str::to_string),
        rotating_market_family: Some("updown".to_string()),
        underlying_asset: Some("ETH".to_string()),
        cadence_seconds: Some(300),
        market_selection_rule: Some("active_or_next".to_string()),
        retry_interval_seconds: Some(5),
        blocked_after_seconds: Some(60),
        polymarket_condition_id: None,
        polymarket_market_slug: None,
        polymarket_question_id: None,
        up_instrument_id: None,
        down_instrument_id: None,
        selected_market_observed_timestamp: None,
        polymarket_market_start_timestamp_milliseconds: None,
        polymarket_market_end_timestamp_milliseconds: None,
        price_to_beat_value: None,
        price_to_beat_observed_timestamp: None,
        price_to_beat_source: None,
    }
}

fn entry_evaluation_facts() -> BoltV3EntryEvaluationFacts {
    BoltV3EntryEvaluationFacts {
        updown_side: Some("up".to_string()),
        entry_decision: "enter".to_string(),
        entry_no_action_reason: None,
        seconds_to_market_end: 240,
        has_selected_market_open_orders: false,
        updown_market_mechanical_outcome: "accepted".to_string(),
        updown_market_mechanical_rejection_reason: None,
        entry_filled_notional: 0.0,
        open_entry_notional: 0.0,
        strategy_remaining_entry_capacity: 100.0,
        archetype_metrics: json!({"expected_edge_basis_points": 42.0}),
    }
}

fn exit_evaluation_facts() -> BoltV3ExitEvaluationFacts {
    BoltV3ExitEvaluationFacts {
        authoritative_position_quantity: Some(10.0),
        authoritative_sellable_quantity: Some(10.0),
        open_exit_order_quantity: Some(0.0),
        uncovered_position_quantity: Some(10.0),
        exit_order_mechanical_outcome: "accepted".to_string(),
        exit_order_mechanical_rejection_reason: None,
        exit_decision: "hold".to_string(),
        exit_decision_reason: "active_exit_not_defined".to_string(),
        archetype_metrics: json!({}),
    }
}

fn order_submission_facts(client_order_id: Option<String>) -> BoltV3OrderSubmissionFacts {
    BoltV3OrderSubmissionFacts {
        order_type: "limit".to_string(),
        time_in_force: "gtc".to_string(),
        instrument_id: "ETH-UP.POLYMARKET".to_string(),
        side: "buy".to_string(),
        price: 0.52,
        quantity: 10.0,
        is_quote_quantity: false,
        is_post_only: false,
        is_reduce_only: false,
        client_order_id,
    }
}

fn query_events(path: &std::path::Path, event_type: &str) -> Vec<Data> {
    let ids = vec!["target-eth-updown".to_string()];
    ParquetDataCatalog::new(path, None, None, None, None)
        .query_custom_data_dynamic(event_type, Some(&ids), None, None, None, None, true)
        .unwrap()
}

fn persistence_block(path: impl AsRef<std::path::Path>) -> PersistenceBlock {
    PersistenceBlock {
        catalog_directory: path.as_ref().to_string_lossy().into_owned(),
        streaming: StreamingBlock {
            catalog_fs_protocol: CatalogFsProtocol::File,
            flush_interval_milliseconds: 1,
            replace_existing: false,
            rotation_kind: RotationKind::None,
        },
    }
}
