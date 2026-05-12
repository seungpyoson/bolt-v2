mod support;

use std::{cell::Cell, fs};

use bolt_v2::bolt_v3_config::{
    CatalogFsProtocol, PersistenceBlock, RotationKind, StreamingBlock, load_bolt_v3_config,
};
use bolt_v2::bolt_v3_decision_event_context::bolt_v3_decision_event_common_fields;
use bolt_v2::bolt_v3_decision_events::{
    BOLT_V3_ENTRY_ORDER_SUBMISSION_DECISION_EVENT_TYPE, BoltV3DecisionEventCatalogHandoff,
    BoltV3DecisionEventCommonFields, BoltV3EntryOrderSubmissionDecisionEvent,
    BoltV3ExitOrderSubmissionDecisionEvent,
};
use bolt_v2::bolt_v3_order_intent_gate::{gate_entry_order_submission, gate_exit_order_submission};
use bolt_v2::bolt_v3_release_identity::load_bolt_v3_release_identity;
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
    let common = common_fields(temp_dir.path());
    let target_id = common.configured_target_id.clone();
    let event = entry_order_submission_event(common);

    let result = gate_entry_order_submission(&mut handoff, event, || {
        submit_called.set(true);
        Ok("submitted")
    })
    .unwrap();

    assert_eq!(result, "submitted");
    assert!(submit_called.get());
    assert_eq!(query_entry_events(temp_dir.path(), &target_id).len(), 1);
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
    let event = entry_order_submission_event(common_fields(temp_dir.path()));

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
    let event = exit_order_submission_event(common_fields(temp_dir.path()));

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

fn entry_order_submission_event(
    common: BoltV3DecisionEventCommonFields,
) -> BoltV3EntryOrderSubmissionDecisionEvent {
    BoltV3EntryOrderSubmissionDecisionEvent::entry_order_submission(
        common,
        support::bolt_v3_order_submission_facts_fixture("entry_order_submission_facts.json"),
        entry_order_submission_event_ts(),
        entry_order_submission_init_ts(),
    )
    .unwrap()
}

fn exit_order_submission_event(
    common: BoltV3DecisionEventCommonFields,
) -> BoltV3ExitOrderSubmissionDecisionEvent {
    BoltV3ExitOrderSubmissionDecisionEvent::exit_order_submission(
        common,
        support::bolt_v3_order_submission_facts_fixture("exit_order_submission_facts.json"),
        exit_order_submission_event_ts(),
        exit_order_submission_init_ts(),
    )
    .unwrap()
}

fn decision_event_timestamps() -> support::BoltV3DecisionEventTimestampsFixture {
    support::bolt_v3_decision_event_timestamps_fixture("event_timestamps.json")
}

fn entry_order_submission_event_ts() -> UnixNanos {
    UnixNanos::from(
        decision_event_timestamps()
            .entry_order_submission
            .event_ts_nanos,
    )
}

fn entry_order_submission_init_ts() -> UnixNanos {
    UnixNanos::from(
        decision_event_timestamps()
            .entry_order_submission
            .init_ts_nanos,
    )
}

fn exit_order_submission_event_ts() -> UnixNanos {
    UnixNanos::from(
        decision_event_timestamps()
            .exit_order_submission
            .event_ts_nanos,
    )
}

fn exit_order_submission_init_ts() -> UnixNanos {
    UnixNanos::from(
        decision_event_timestamps()
            .exit_order_submission
            .init_ts_nanos,
    )
}

fn common_fields(temp_dir: &std::path::Path) -> BoltV3DecisionEventCommonFields {
    let mut loaded = load_bolt_v3_config(&support::repo_path(
        "tests/fixtures/bolt_v3_existing_strategy/root.toml",
    ))
    .expect("existing strategy root should load");
    support::attach_test_release_identity_manifest(&mut loaded, temp_dir);
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

fn query_entry_events(path: &std::path::Path, target_id: &str) -> Vec<Data> {
    let ids = vec![target_id.to_string()];
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
