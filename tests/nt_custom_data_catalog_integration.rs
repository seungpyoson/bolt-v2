#![allow(unexpected_cfgs)]

use std::sync::Arc;

use nautilus_core::UnixNanos;
use nautilus_model::{
    data::{CustomData, Data, DataType},
    identifiers::InstrumentId,
};
use nautilus_persistence::backend::catalog::ParquetDataCatalog;
use nautilus_persistence_macros::custom_data;
use nautilus_serialization::ensure_custom_data_registered;
use tempfile::TempDir;

#[custom_data]
struct VerificationDecisionEvent {
    instrument_id: InstrumentId,
    decision_trace_identifier: String,
    event_kind: String,
    ts_event: UnixNanos,
    ts_init: UnixNanos,
}

#[test]
fn pinned_custom_data_round_trips_through_local_catalog() {
    ensure_custom_data_registered::<VerificationDecisionEvent>();

    let temp_dir = TempDir::new().unwrap();
    let mut catalog = ParquetDataCatalog::new(temp_dir.path(), None, None, None, None);

    let instrument_id = InstrumentId::from("BTCUSDT.BINANCE");
    let data_type = DataType::new(
        "VerificationDecisionEvent",
        None,
        Some(instrument_id.to_string()),
    );

    let original = VerificationDecisionEvent {
        instrument_id,
        decision_trace_identifier: "123e4567-e89b-12d3-a456-426614174000".to_string(),
        event_kind: "entry_evaluation".to_string(),
        ts_event: UnixNanos::from(100),
        ts_init: UnixNanos::from(100),
    };

    let custom = CustomData::new(Arc::new(original.clone()), data_type);
    catalog
        .write_custom_data_batch(vec![custom], None, None, Some(false))
        .unwrap();

    let ids = vec![instrument_id.to_string()];
    let loaded: Vec<Data> = catalog
        .query_custom_data_dynamic(
            "VerificationDecisionEvent",
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
        Data::Custom(decoded) => {
            assert_eq!(decoded.data_type.type_name(), "VerificationDecisionEvent");
            assert_eq!(
                decoded.data_type.identifier(),
                Some(instrument_id.to_string().as_str())
            );
        }
        other => panic!("expected Data::Custom, got {other:?}"),
    }
}
