mod support;

use bolt_v2::bolt_v3_config::{
    CatalogFsProtocol, PersistenceBlock, RotationKind, StreamingBlock, load_bolt_v3_config,
};
use bolt_v2::bolt_v3_decision_event_context::bolt_v3_decision_event_common_fields;
use bolt_v2::bolt_v3_decision_events::{
    BOLT_V3_ARCHETYPE_METRICS_FACT_KEY, BOLT_V3_CLIENT_ORDER_ID_FACT_KEY,
    BOLT_V3_ENTRY_EVALUATION_DECISION_EVENT_TYPE, BOLT_V3_ENTRY_EVALUATION_EVENT_VALUE,
    BOLT_V3_ENTRY_NO_ACTION_ACTIVE_BOOK_NOT_PRICED_REASON,
    BOLT_V3_ENTRY_NO_ACTION_FAST_VENUE_INCOHERENT_REASON, BOLT_V3_ENTRY_NO_ACTION_FREEZE_REASON,
    BOLT_V3_ENTRY_NO_ACTION_INSUFFICIENT_EDGE_REASON,
    BOLT_V3_ENTRY_NO_ACTION_MARKET_COOLING_DOWN_REASON,
    BOLT_V3_ENTRY_NO_ACTION_METADATA_MISMATCH_REASON,
    BOLT_V3_ENTRY_NO_ACTION_ONE_POSITION_INVARIANT_REASON, BOLT_V3_ENTRY_NO_ACTION_REASON_FACT_KEY,
    BOLT_V3_ENTRY_NO_ACTION_RECOVERY_MODE_REASON, BOLT_V3_ENTRY_NO_ACTION_THIN_BOOK_REASON,
    BOLT_V3_ENTRY_ORDER_SUBMISSION_DECISION_EVENT_TYPE,
    BOLT_V3_ENTRY_PRE_SUBMIT_REJECTION_DECISION_EVENT_TYPE,
    BOLT_V3_ENTRY_PRE_SUBMIT_REJECTION_INSTRUMENT_ID_MISSING_REASON,
    BOLT_V3_ENTRY_PRE_SUBMIT_REJECTION_INVALID_QUANTITY_REASON,
    BOLT_V3_ENTRY_PRE_SUBMIT_REJECTION_REASON_FACT_KEY, BOLT_V3_ENTRY_PRE_SUBMIT_REJECTION_REASONS,
    BOLT_V3_EXIT_DECISION_ORDER_MECHANICAL_REJECTION_REASON,
    BOLT_V3_EXIT_EVALUATION_DECISION_EVENT_TYPE, BOLT_V3_EXIT_EVALUATION_EVENT_VALUE,
    BOLT_V3_EXIT_ORDER_MECHANICAL_REJECTION_REASON_FACT_KEY,
    BOLT_V3_EXIT_ORDER_SUBMISSION_DECISION_EVENT_TYPE,
    BOLT_V3_EXIT_PRE_SUBMIT_REJECTION_DECISION_EVENT_TYPE,
    BOLT_V3_EXIT_PRE_SUBMIT_REJECTION_INVALID_QUANTITY_REASON,
    BOLT_V3_EXIT_PRE_SUBMIT_REJECTION_REASON_FACT_KEY, BOLT_V3_EXIT_PRE_SUBMIT_REJECTION_REASONS,
    BOLT_V3_MARKET_SELECTION_DECISION_EVENT_TYPE, BOLT_V3_MARKET_SELECTION_FAILURE_REASON_FACT_KEY,
    BOLT_V3_MARKET_SELECTION_FAILURE_REASONS, BoltV3DecisionEventCatalogHandoff,
    BoltV3DecisionEventCommonFields, BoltV3EntryEvaluationDecisionEvent,
    BoltV3EntryEvaluationFacts, BoltV3EntryOrderSubmissionDecisionEvent,
    BoltV3EntryPreSubmitRejectionDecisionEvent, BoltV3ExitEvaluationDecisionEvent,
    BoltV3ExitEvaluationFacts, BoltV3ExitOrderSubmissionDecisionEvent,
    BoltV3ExitPreSubmitRejectionDecisionEvent, BoltV3MarketSelectionDecisionEvent,
    BoltV3MarketSelectionResultFacts, BoltV3OrderSubmissionFacts, BoltV3PreSubmitRejectionFacts,
    BoltV3RejectedOrderFacts, register_bolt_v3_decision_event_types,
};
use bolt_v2::bolt_v3_release_identity::load_bolt_v3_release_identity;
use bolt_v2::platform::polymarket_catalog::POLYMARKET_GAMMA_MARKET_ANCHOR_SOURCE;
use nautilus_core::UnixNanos;
use nautilus_model::data::Data;
use nautilus_persistence::backend::catalog::ParquetDataCatalog;
use serde_json::Value;
use std::fs;
use tempfile::TempDir;

const TEST_ENTRY_EVALUATION_EVENT_TS_NANOS: u64 = 2_500;
const TEST_ENTRY_EVALUATION_INIT_TS_NANOS: u64 = 2_501;
const TEST_UNSUPPORTED_DECISION_REASON: &str = "some_new_reason";

fn decision_event_json_fixture(filename: &str) -> Value {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/bolt_v3_decision_events")
        .join(filename);
    let body = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("decision-event fixture {filename} should load: {error}"));
    serde_json::from_str(&body)
        .unwrap_or_else(|error| panic!("decision-event fixture {filename} should parse: {error}"))
}

fn entry_archetype_metrics() -> Value {
    decision_event_json_fixture("entry_archetype_metrics.json")
}

fn exit_archetype_metrics() -> Value {
    decision_event_json_fixture("exit_archetype_metrics.json")
}

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
            price_to_beat_source: Some(POLYMARKET_GAMMA_MARKET_ANCHOR_SOURCE.to_string()),
        },
        UnixNanos::from(2_000),
        UnixNanos::from(2_001),
    )
    .unwrap();

    handoff.write_market_selection_result(event).unwrap();

    let ids = test_target_ids();
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
            assert_eq!(
                decoded.configured_target_id,
                common_fields().configured_target_id
            );
            assert_eq!(
                decoded
                    .event_facts
                    .get(BOLT_V3_MARKET_SELECTION_FAILURE_REASON_FACT_KEY),
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
        failed_market_selection_result_facts(Some(TEST_UNSUPPORTED_DECISION_REASON)),
        UnixNanos::from(2_000),
        UnixNanos::from(2_001),
    )
    .unwrap_err();

    assert!(error.to_string().contains(&format!(
        "unsupported market_selection_failure_reason `{TEST_UNSUPPORTED_DECISION_REASON}`"
    )));
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
            price_to_beat_source: Some(POLYMARKET_GAMMA_MARKET_ANCHOR_SOURCE.to_string()),
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
        test_entry_evaluation_event_ts(),
        test_entry_evaluation_init_ts(),
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
            assert_eq!(
                decoded.decision_event_type,
                BOLT_V3_ENTRY_EVALUATION_EVENT_VALUE
            );
            assert_eq!(
                decoded
                    .event_facts
                    .get(BOLT_V3_ENTRY_NO_ACTION_REASON_FACT_KEY),
                Some(&Value::Null)
            );
            assert_eq!(
                decoded.event_facts.get(BOLT_V3_ARCHETYPE_METRICS_FACT_KEY),
                Some(&entry_archetype_metrics())
            );
        }
        other => panic!("expected Data::Custom, got {other:?}"),
    }
}

#[test]
fn entry_evaluation_handoff_preserves_same_timestamp_events() {
    let temp_dir = TempDir::new().unwrap();
    let mut handoff = BoltV3DecisionEventCatalogHandoff::from_persistence_block(
        &persistence_block(temp_dir.path()),
    )
    .unwrap();

    for decision in ["enter", "no_action"] {
        let mut facts = entry_evaluation_facts();
        facts.entry_decision = decision.to_string();
        facts.entry_no_action_reason = (decision == "no_action")
            .then(|| BOLT_V3_ENTRY_NO_ACTION_INSUFFICIENT_EDGE_REASON.to_string());
        facts.updown_side = (decision == "enter").then(|| "up".to_string());

        let event = BoltV3EntryEvaluationDecisionEvent::entry_evaluation(
            common_fields(),
            facts,
            test_entry_evaluation_event_ts(),
            test_entry_evaluation_init_ts(),
        )
        .unwrap();

        handoff.write_entry_evaluation(event).unwrap();
    }

    let loaded = query_events(
        temp_dir.path(),
        BOLT_V3_ENTRY_EVALUATION_DECISION_EVENT_TYPE,
    );

    assert_eq!(loaded.len(), 2);

    let ids = test_target_ids();
    let loaded_by_event_time = ParquetDataCatalog::new(temp_dir.path(), None, None, None, None)
        .query_custom_data_dynamic(
            BOLT_V3_ENTRY_EVALUATION_DECISION_EVENT_TYPE,
            Some(&ids),
            Some(test_entry_evaluation_event_ts()),
            Some(test_entry_evaluation_init_ts()),
            None,
            None,
            true,
        )
        .unwrap();
    assert_eq!(loaded_by_event_time.len(), 2);
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
        test_entry_evaluation_event_ts(),
        test_entry_evaluation_init_ts(),
    )
    .unwrap_err();

    assert!(
        error
            .to_string()
            .contains("entry_no_action_reason must be non-null")
    );
}

#[test]
fn entry_evaluation_accepts_market_cooling_down_no_action_reason() {
    let mut facts = entry_evaluation_facts();
    facts.entry_decision = "no_action".to_string();
    facts.updown_side = None;
    facts.entry_no_action_reason =
        Some(BOLT_V3_ENTRY_NO_ACTION_MARKET_COOLING_DOWN_REASON.to_string());

    let event = BoltV3EntryEvaluationDecisionEvent::entry_evaluation(
        common_fields(),
        facts,
        test_entry_evaluation_event_ts(),
        test_entry_evaluation_init_ts(),
    )
    .unwrap();

    assert_eq!(
        event
            .event_facts
            .get(BOLT_V3_ENTRY_NO_ACTION_REASON_FACT_KEY),
        Some(&Value::String(
            BOLT_V3_ENTRY_NO_ACTION_MARKET_COOLING_DOWN_REASON.to_string()
        ))
    );
}

#[test]
fn entry_evaluation_accepts_recovery_mode_no_action_reason() {
    let mut facts = entry_evaluation_facts();
    facts.entry_decision = "no_action".to_string();
    facts.updown_side = None;
    facts.entry_no_action_reason = Some(BOLT_V3_ENTRY_NO_ACTION_RECOVERY_MODE_REASON.to_string());

    let event = BoltV3EntryEvaluationDecisionEvent::entry_evaluation(
        common_fields(),
        facts,
        test_entry_evaluation_event_ts(),
        test_entry_evaluation_init_ts(),
    )
    .unwrap();

    assert_eq!(
        event
            .event_facts
            .get(BOLT_V3_ENTRY_NO_ACTION_REASON_FACT_KEY),
        Some(&Value::String(
            BOLT_V3_ENTRY_NO_ACTION_RECOVERY_MODE_REASON.to_string()
        ))
    );
}

#[test]
fn entry_evaluation_accepts_one_position_invariant_no_action_reason() {
    let mut facts = entry_evaluation_facts();
    facts.entry_decision = "no_action".to_string();
    facts.updown_side = None;
    facts.entry_no_action_reason =
        Some(BOLT_V3_ENTRY_NO_ACTION_ONE_POSITION_INVARIANT_REASON.to_string());

    let event = BoltV3EntryEvaluationDecisionEvent::entry_evaluation(
        common_fields(),
        facts,
        test_entry_evaluation_event_ts(),
        test_entry_evaluation_init_ts(),
    )
    .unwrap();

    assert_eq!(
        event
            .event_facts
            .get(BOLT_V3_ENTRY_NO_ACTION_REASON_FACT_KEY),
        Some(&Value::String(
            BOLT_V3_ENTRY_NO_ACTION_ONE_POSITION_INVARIANT_REASON.to_string()
        ))
    );
}

#[test]
fn entry_evaluation_accepts_thin_book_no_action_reason() {
    let mut facts = entry_evaluation_facts();
    facts.entry_decision = "no_action".to_string();
    facts.updown_side = None;
    facts.entry_no_action_reason = Some(BOLT_V3_ENTRY_NO_ACTION_THIN_BOOK_REASON.to_string());

    let event = BoltV3EntryEvaluationDecisionEvent::entry_evaluation(
        common_fields(),
        facts,
        test_entry_evaluation_event_ts(),
        test_entry_evaluation_init_ts(),
    )
    .unwrap();

    assert_eq!(
        event
            .event_facts
            .get(BOLT_V3_ENTRY_NO_ACTION_REASON_FACT_KEY),
        Some(&Value::String(
            BOLT_V3_ENTRY_NO_ACTION_THIN_BOOK_REASON.to_string()
        ))
    );
}

#[test]
fn entry_evaluation_accepts_fast_venue_incoherent_no_action_reason() {
    let mut facts = entry_evaluation_facts();
    facts.entry_decision = "no_action".to_string();
    facts.updown_side = None;
    facts.entry_no_action_reason =
        Some(BOLT_V3_ENTRY_NO_ACTION_FAST_VENUE_INCOHERENT_REASON.to_string());

    let event = BoltV3EntryEvaluationDecisionEvent::entry_evaluation(
        common_fields(),
        facts,
        test_entry_evaluation_event_ts(),
        test_entry_evaluation_init_ts(),
    )
    .unwrap();

    assert_eq!(
        event
            .event_facts
            .get(BOLT_V3_ENTRY_NO_ACTION_REASON_FACT_KEY),
        Some(&Value::String(
            BOLT_V3_ENTRY_NO_ACTION_FAST_VENUE_INCOHERENT_REASON.to_string()
        ))
    );
}

#[test]
fn entry_evaluation_accepts_freeze_no_action_reason() {
    let mut facts = entry_evaluation_facts();
    facts.entry_decision = "no_action".to_string();
    facts.updown_side = None;
    facts.entry_no_action_reason = Some(BOLT_V3_ENTRY_NO_ACTION_FREEZE_REASON.to_string());

    let event = BoltV3EntryEvaluationDecisionEvent::entry_evaluation(
        common_fields(),
        facts,
        test_entry_evaluation_event_ts(),
        test_entry_evaluation_init_ts(),
    )
    .unwrap();

    assert_eq!(
        event
            .event_facts
            .get(BOLT_V3_ENTRY_NO_ACTION_REASON_FACT_KEY),
        Some(&Value::String(
            BOLT_V3_ENTRY_NO_ACTION_FREEZE_REASON.to_string()
        ))
    );
}

#[test]
fn entry_evaluation_accepts_book_state_no_action_reasons() {
    for reason in [
        BOLT_V3_ENTRY_NO_ACTION_ACTIVE_BOOK_NOT_PRICED_REASON,
        BOLT_V3_ENTRY_NO_ACTION_METADATA_MISMATCH_REASON,
    ] {
        let mut facts = entry_evaluation_facts();
        facts.entry_decision = "no_action".to_string();
        facts.entry_no_action_reason = Some(reason.to_string());
        facts.updown_side = None;

        let event = BoltV3EntryEvaluationDecisionEvent::entry_evaluation(
            common_fields(),
            facts,
            UnixNanos::from(TEST_ENTRY_EVALUATION_EVENT_TS_NANOS),
            UnixNanos::from(TEST_ENTRY_EVALUATION_INIT_TS_NANOS),
        )
        .unwrap();

        assert_eq!(
            event
                .event_facts
                .get(BOLT_V3_ENTRY_NO_ACTION_REASON_FACT_KEY),
            Some(&Value::String(reason.to_string()))
        );
    }
}

#[test]
fn entry_order_submission_event_writes_through_nt_catalog_handoff() {
    let temp_dir = TempDir::new().unwrap();
    let mut handoff = BoltV3DecisionEventCatalogHandoff::from_persistence_block(
        &persistence_block(temp_dir.path()),
    )
    .unwrap();

    let order_facts = order_submission_facts_from_fixture();
    let expected_client_order_id = order_facts
        .client_order_id
        .clone()
        .expect("entry order fixture should define client_order_id");
    let event = BoltV3EntryOrderSubmissionDecisionEvent::entry_order_submission(
        common_fields(),
        order_facts,
        UnixNanos::from(3_000),
        UnixNanos::from(3_001),
    )
    .unwrap();

    handoff.write_entry_order_submission(event).unwrap();

    let ids = test_target_ids();
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
                decoded.event_facts.get(BOLT_V3_CLIENT_ORDER_ID_FACT_KEY),
                Some(&Value::String(expected_client_order_id))
            );
        }
        other => panic!("expected Data::Custom, got {other:?}"),
    }
}

#[test]
fn entry_order_submission_rejects_missing_client_order_id() {
    let error = BoltV3EntryOrderSubmissionDecisionEvent::entry_order_submission(
        common_fields(),
        order_submission_facts_without_client_order_id(),
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
            order: BoltV3RejectedOrderFacts::from(order_submission_facts_without_client_order_id()),
            rejection_reason: BOLT_V3_ENTRY_PRE_SUBMIT_REJECTION_INVALID_QUANTITY_REASON
                .to_string(),
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

    let ids = test_target_ids();
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
                decoded.event_facts.get(BOLT_V3_CLIENT_ORDER_ID_FACT_KEY),
                Some(&Value::Null)
            );
            assert_eq!(
                decoded
                    .event_facts
                    .get(BOLT_V3_ENTRY_PRE_SUBMIT_REJECTION_REASON_FACT_KEY),
                Some(&Value::String(
                    BOLT_V3_ENTRY_PRE_SUBMIT_REJECTION_INVALID_QUANTITY_REASON.to_string()
                ))
            );
        }
        other => panic!("expected Data::Custom, got {other:?}"),
    }
}

#[test]
fn entry_pre_submit_rejection_rejects_unknown_reason() {
    let error = BoltV3EntryPreSubmitRejectionDecisionEvent::entry_pre_submit_rejection(
        common_fields(),
        BoltV3PreSubmitRejectionFacts {
            order: BoltV3RejectedOrderFacts::from(order_submission_facts_without_client_order_id()),
            rejection_reason: TEST_UNSUPPORTED_DECISION_REASON.to_string(),
            authoritative_position_quantity: None,
            authoritative_sellable_quantity: None,
            open_exit_order_quantity: None,
            uncovered_position_quantity: None,
        },
        UnixNanos::from(4_000),
        UnixNanos::from(4_001),
    )
    .unwrap_err();

    assert!(error.to_string().contains(&format!(
        "unsupported entry_pre_submit_rejection_reason `{TEST_UNSUPPORTED_DECISION_REASON}`"
    )));
}

#[test]
fn entry_pre_submit_rejection_accepts_allowed_reasons() {
    for reason in BOLT_V3_ENTRY_PRE_SUBMIT_REJECTION_REASONS {
        BoltV3EntryPreSubmitRejectionDecisionEvent::entry_pre_submit_rejection(
            common_fields(),
            BoltV3PreSubmitRejectionFacts {
                order: BoltV3RejectedOrderFacts::from(
                    order_submission_facts_without_client_order_id(),
                ),
                rejection_reason: (*reason).to_string(),
                authoritative_position_quantity: None,
                authoritative_sellable_quantity: None,
                open_exit_order_quantity: None,
                uncovered_position_quantity: None,
            },
            UnixNanos::from(4_000),
            UnixNanos::from(4_001),
        )
        .unwrap();
    }
}

#[test]
fn entry_pre_submit_rejection_accepts_missing_instrument_id_reason() {
    BoltV3EntryPreSubmitRejectionDecisionEvent::entry_pre_submit_rejection(
        common_fields(),
        BoltV3PreSubmitRejectionFacts {
            order: BoltV3RejectedOrderFacts::from(order_submission_facts_without_client_order_id()),
            rejection_reason: BOLT_V3_ENTRY_PRE_SUBMIT_REJECTION_INSTRUMENT_ID_MISSING_REASON
                .to_string(),
            authoritative_position_quantity: None,
            authoritative_sellable_quantity: None,
            open_exit_order_quantity: None,
            uncovered_position_quantity: None,
        },
        UnixNanos::from(4_000),
        UnixNanos::from(4_001),
    )
    .unwrap();
}

#[test]
fn exit_order_submission_event_writes_through_nt_catalog_handoff() {
    let temp_dir = TempDir::new().unwrap();
    let mut handoff = BoltV3DecisionEventCatalogHandoff::from_persistence_block(
        &persistence_block(temp_dir.path()),
    )
    .unwrap();

    let order_facts = exit_order_submission_facts_from_fixture();
    let expected_client_order_id = order_facts
        .client_order_id
        .clone()
        .expect("exit order fixture should define client_order_id");
    let event = BoltV3ExitOrderSubmissionDecisionEvent::exit_order_submission(
        common_fields(),
        order_facts,
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
                decoded.event_facts.get(BOLT_V3_CLIENT_ORDER_ID_FACT_KEY),
                Some(&Value::String(expected_client_order_id))
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
            order: BoltV3RejectedOrderFacts::from(
                exit_order_submission_facts_without_client_order_id(),
            ),
            rejection_reason: BOLT_V3_EXIT_PRE_SUBMIT_REJECTION_INVALID_QUANTITY_REASON.to_string(),
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
                decoded.event_facts.get(BOLT_V3_CLIENT_ORDER_ID_FACT_KEY),
                Some(&Value::Null)
            );
            assert_eq!(
                decoded
                    .event_facts
                    .get(BOLT_V3_EXIT_PRE_SUBMIT_REJECTION_REASON_FACT_KEY),
                Some(&Value::String(
                    BOLT_V3_EXIT_PRE_SUBMIT_REJECTION_INVALID_QUANTITY_REASON.to_string()
                ))
            );
        }
        other => panic!("expected Data::Custom, got {other:?}"),
    }
}

#[test]
fn exit_pre_submit_rejection_rejects_unknown_reason() {
    let error = BoltV3ExitPreSubmitRejectionDecisionEvent::exit_pre_submit_rejection(
        common_fields(),
        BoltV3PreSubmitRejectionFacts {
            order: BoltV3RejectedOrderFacts::from(
                exit_order_submission_facts_without_client_order_id(),
            ),
            rejection_reason: TEST_UNSUPPORTED_DECISION_REASON.to_string(),
            authoritative_position_quantity: Some(10.0),
            authoritative_sellable_quantity: Some(10.0),
            open_exit_order_quantity: Some(0.0),
            uncovered_position_quantity: Some(10.0),
        },
        UnixNanos::from(6_000),
        UnixNanos::from(6_001),
    )
    .unwrap_err();

    assert!(error.to_string().contains(&format!(
        "unsupported exit_pre_submit_rejection_reason `{TEST_UNSUPPORTED_DECISION_REASON}`"
    )));
}

#[test]
fn exit_pre_submit_rejection_accepts_allowed_reasons() {
    for reason in BOLT_V3_EXIT_PRE_SUBMIT_REJECTION_REASONS {
        BoltV3ExitPreSubmitRejectionDecisionEvent::exit_pre_submit_rejection(
            common_fields(),
            BoltV3PreSubmitRejectionFacts {
                order: BoltV3RejectedOrderFacts::from(
                    exit_order_submission_facts_without_client_order_id(),
                ),
                rejection_reason: (*reason).to_string(),
                authoritative_position_quantity: Some(10.0),
                authoritative_sellable_quantity: Some(10.0),
                open_exit_order_quantity: Some(0.0),
                uncovered_position_quantity: Some(10.0),
            },
            UnixNanos::from(6_000),
            UnixNanos::from(6_001),
        )
        .unwrap();
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
            assert_eq!(
                decoded.decision_event_type,
                BOLT_V3_EXIT_EVALUATION_EVENT_VALUE
            );
            assert_eq!(
                decoded
                    .event_facts
                    .get(BOLT_V3_EXIT_ORDER_MECHANICAL_REJECTION_REASON_FACT_KEY),
                Some(&Value::Null)
            );
            assert_eq!(
                decoded.event_facts.get(BOLT_V3_ARCHETYPE_METRICS_FACT_KEY),
                Some(&exit_archetype_metrics())
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
    facts.exit_decision_reason =
        BOLT_V3_EXIT_DECISION_ORDER_MECHANICAL_REJECTION_REASON.to_string();

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
    let temp_dir = TempDir::new().unwrap();
    let mut loaded = load_bolt_v3_config(&support::repo_path(
        "tests/fixtures/bolt_v3_existing_strategy/root.toml",
    ))
    .expect("existing strategy root should load");
    support::attach_test_release_identity_manifest(&mut loaded, temp_dir.path());
    let identity = load_bolt_v3_release_identity(&loaded).expect("release identity should load");
    let strategy = loaded
        .strategies
        .first()
        .expect("existing strategy root should load one strategy");
    let decision_trace_id = format!(
        "{}:{}",
        identity.release_id, strategy.config.strategy_instance_id
    );

    bolt_v3_decision_event_common_fields(&loaded, strategy, &identity, &decision_trace_id)
        .expect("common fields should derive from v3 TOML and release identity")
}

fn test_target_ids() -> Vec<String> {
    vec![common_fields().configured_target_id]
}

fn test_entry_evaluation_event_ts() -> UnixNanos {
    UnixNanos::from(TEST_ENTRY_EVALUATION_EVENT_TS_NANOS)
}

fn test_entry_evaluation_init_ts() -> UnixNanos {
    UnixNanos::from(TEST_ENTRY_EVALUATION_INIT_TS_NANOS)
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
    support::bolt_v3_entry_evaluation_facts_fixture("entry_evaluation_facts.json")
}

fn exit_evaluation_facts() -> BoltV3ExitEvaluationFacts {
    support::bolt_v3_exit_evaluation_facts_fixture("exit_evaluation_facts.json")
}

fn order_submission_facts_from_fixture() -> BoltV3OrderSubmissionFacts {
    support::bolt_v3_order_submission_facts_fixture("entry_order_submission_facts.json")
}

fn order_submission_facts_without_client_order_id() -> BoltV3OrderSubmissionFacts {
    let mut facts = order_submission_facts_from_fixture();
    facts.client_order_id = None;
    facts
}

fn exit_order_submission_facts_from_fixture() -> BoltV3OrderSubmissionFacts {
    support::bolt_v3_order_submission_facts_fixture("exit_order_submission_facts.json")
}

fn exit_order_submission_facts_without_client_order_id() -> BoltV3OrderSubmissionFacts {
    let mut facts = exit_order_submission_facts_from_fixture();
    facts.client_order_id = None;
    facts
}

fn query_events(path: &std::path::Path, event_type: &str) -> Vec<Data> {
    let ids = test_target_ids();
    let event_dir = path
        .join("data")
        .join("custom")
        .join(event_type)
        .join(&ids[0]);
    let mut files = fs::read_dir(event_dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "parquet")
        })
        .map(|path| path.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    files.sort();

    files
        .into_iter()
        .flat_map(|file| {
            ParquetDataCatalog::new(path, None, None, None, None)
                .query_custom_data_dynamic(
                    event_type,
                    Some(&ids),
                    None,
                    None,
                    None,
                    Some(vec![file]),
                    true,
                )
                .unwrap()
        })
        .collect()
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
