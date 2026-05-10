use bolt_v2::bolt_v3_config::{CatalogFsProtocol, PersistenceBlock, RotationKind, StreamingBlock};
use bolt_v2::bolt_v3_decision_events::{
    BOLT_V3_MARKET_SELECTION_DECISION_EVENT_TYPE, BoltV3DecisionEventCatalogHandoff,
    BoltV3DecisionEventCommonFields, BoltV3MarketSelectionDecisionEvent,
    BoltV3MarketSelectionResultFacts, register_bolt_v3_decision_event_types,
};
use nautilus_core::UnixNanos;
use nautilus_model::data::Data;
use nautilus_persistence::backend::catalog::ParquetDataCatalog;
use serde_json::Value;
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
        BoltV3MarketSelectionResultFacts {
            market_selection_type: "rotating_market".to_string(),
            market_selection_timestamp_milliseconds: 1_000,
            market_selection_outcome: "failed".to_string(),
            market_selection_failure_reason: None,
        },
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
        },
        UnixNanos::from(2_000),
        UnixNanos::from(2_001),
    )
    .unwrap();

    let error = handoff.write_market_selection_result(event).unwrap_err();

    assert!(!error.to_string().is_empty());
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
