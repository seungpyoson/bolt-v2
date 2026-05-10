mod support;

use std::{cell::Cell, fs};

use bolt_v2::bolt_v3_config::{CatalogFsProtocol, PersistenceBlock, RotationKind, StreamingBlock};
use bolt_v2::bolt_v3_decision_events::{
    BOLT_V3_ENTRY_ORDER_SUBMISSION_DECISION_EVENT_TYPE, BoltV3DecisionEventCatalogHandoff,
    BoltV3DecisionEventCommonFields, BoltV3EntryOrderSubmissionDecisionEvent,
    BoltV3ExitOrderSubmissionDecisionEvent, BoltV3OrderSubmissionFacts,
};
use bolt_v2::bolt_v3_order_intent_gate::{gate_entry_order_submission, gate_exit_order_submission};
use nautilus_core::UnixNanos;
use nautilus_model::data::Data;
use nautilus_persistence::backend::catalog::ParquetDataCatalog;
use tempfile::TempDir;

#[test]
fn order_intent_gate_persists_entry_event_before_submit_closure() {
    let temp_dir = TempDir::new().unwrap();
    let mut handoff = BoltV3DecisionEventCatalogHandoff::from_persistence_block(
        &persistence_block(temp_dir.path()),
    )
    .unwrap();
    let submit_called = Cell::new(false);
    let event = entry_order_submission_event();

    let result = gate_entry_order_submission(&mut handoff, event, || {
        submit_called.set(true);
        Ok("submitted")
    })
    .unwrap();

    assert_eq!(result, "submitted");
    assert!(submit_called.get());
    assert_eq!(query_entry_events(temp_dir.path()).len(), 1);
}

#[test]
fn order_intent_gate_blocks_submit_closure_when_handoff_fails() {
    let temp_dir = TempDir::new().unwrap();
    let file_path = temp_dir.path().join("not-a-directory");
    fs::write(&file_path, b"occupied").unwrap();
    let mut handoff =
        BoltV3DecisionEventCatalogHandoff::from_persistence_block(&persistence_block(&file_path))
            .unwrap();
    let submit_called = Cell::new(false);
    let event = entry_order_submission_event();

    let error = gate_entry_order_submission(&mut handoff, event, || {
        submit_called.set(true);
        Ok("submitted")
    })
    .unwrap_err();

    assert!(!error.to_string().is_empty());
    assert!(!submit_called.get());
}

#[test]
fn order_intent_gate_blocks_exit_submit_closure_when_handoff_fails() {
    let temp_dir = TempDir::new().unwrap();
    let file_path = temp_dir.path().join("not-a-directory");
    fs::write(&file_path, b"occupied").unwrap();
    let mut handoff =
        BoltV3DecisionEventCatalogHandoff::from_persistence_block(&persistence_block(&file_path))
            .unwrap();
    let submit_called = Cell::new(false);
    let event = exit_order_submission_event();

    let error = gate_exit_order_submission(&mut handoff, event, || {
        submit_called.set(true);
        Ok("submitted")
    })
    .unwrap_err();

    assert!(!error.to_string().is_empty());
    assert!(!submit_called.get());
}

#[test]
fn order_intent_gate_wiring_has_no_direct_nt_start_run_or_order_api_calls() {
    let path = support::repo_path("src/bolt_v3_order_intent_gate.rs");
    let source = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{}: {error}", path.display()));

    for forbidden in [
        ".start(",
        ".run(",
        "submit_order",
        "submit_order_list",
        "order_factory",
        "subscribe_book",
        "subscribe_quotes",
        "subscribe_trades",
        "subscribe_instruments",
    ] {
        assert!(
            !source.contains(forbidden),
            "bolt-v3 order-intent gate must not call `{forbidden}`"
        );
    }
}

fn entry_order_submission_event() -> BoltV3EntryOrderSubmissionDecisionEvent {
    BoltV3EntryOrderSubmissionDecisionEvent::entry_order_submission(
        common_fields(),
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
            client_order_id: Some("ORDER-001".to_string()),
        },
        UnixNanos::from(3_000),
        UnixNanos::from(3_001),
    )
    .unwrap()
}

fn exit_order_submission_event() -> BoltV3ExitOrderSubmissionDecisionEvent {
    BoltV3ExitOrderSubmissionDecisionEvent::exit_order_submission(
        common_fields(),
        BoltV3OrderSubmissionFacts {
            order_type: "limit".to_string(),
            time_in_force: "fok".to_string(),
            instrument_id: "ETH-UP.POLYMARKET".to_string(),
            side: "sell".to_string(),
            price: 0.48,
            quantity: 10.0,
            is_quote_quantity: false,
            is_post_only: false,
            is_reduce_only: true,
            client_order_id: Some("EXIT-ORDER-001".to_string()),
        },
        UnixNanos::from(4_000),
        UnixNanos::from(4_001),
    )
    .unwrap()
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

fn query_entry_events(path: &std::path::Path) -> Vec<Data> {
    let ids = vec!["target-eth-updown".to_string()];
    ParquetDataCatalog::new(path, None, None, None, None)
        .query_custom_data_dynamic(
            BOLT_V3_ENTRY_ORDER_SUBMISSION_DECISION_EVENT_TYPE,
            Some(&ids),
            None,
            None,
            None,
            None,
            true,
        )
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
