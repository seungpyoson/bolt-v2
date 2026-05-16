use std::{io::Cursor, sync::OnceLock};

use arrow::array::{
    Array, FixedSizeBinaryArray, RecordBatch, StringArray, UInt8Array, UInt32Array, UInt64Array,
};
use arrow::ipc::reader::StreamReader;
use bolt_v2::{
    execution_state::{OrderEventRow, PositionEventRow},
    nt_runtime_capture::spool_root_for_instance,
};
mod support;
use nautilus_common::{
    enums::Environment,
    messages::system::TradingStateChanged,
    msgbus::{
        publish_account_state, publish_any, publish_bar, publish_deltas, publish_depth10,
        publish_funding_rate, publish_index_price, publish_mark_price, publish_order_event,
        publish_position_event, publish_quote, publish_trade, switchboard,
    },
};
use nautilus_core::UUID4;
use nautilus_live::node::LiveNode;
use nautilus_model::{
    data::{
        Bar, BookOrder, FundingRateUpdate, IndexPriceUpdate, InstrumentClose, InstrumentStatus,
        MarkPriceUpdate, OrderBookDelta, OrderBookDeltas, OrderBookDepth10, QuoteTick, TradeTick,
        bar::BarType,
    },
    enums::{
        AccountType, AggressorSide, AssetClass, BarAggregation, BookAction, InstrumentCloseType,
        LiquiditySide, MarketStatusAction, OrderSide, OrderType, PositionAdjustmentType, PriceType,
        TradingState,
    },
    events::{
        AccountState, OrderEventAny, OrderFilled, OrderSubmitted, PositionAdjusted, PositionEvent,
        PositionOpened,
    },
    identifiers::{
        AccountId, ClientOrderId, InstrumentId, PositionId, StrategyId, Symbol, TradeId, TraderId,
        VenueOrderId,
    },
    instruments::{InstrumentAny, binary_option::BinaryOption},
    types::{Currency, Money, Price, Quantity},
};
use support::repo_path;
use tempfile::tempdir;
use tokio::{sync::Mutex, task::LocalSet};

static LIVE_NODE_TEST_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn live_node_test_lock() -> &'static Mutex<()> {
    LIVE_NODE_TEST_LOCK.get_or_init(|| Mutex::new(()))
}

fn collect_paths(root: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut paths = Vec::new();
    let Ok(entries) = std::fs::read_dir(root) else {
        return paths;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        paths.push(path.clone());
        if path.is_dir() {
            paths.extend(collect_paths(&path));
        }
    }

    paths
}

fn find_per_instrument_feather_file(
    spool_root: &std::path::Path,
    type_dir: &str,
    instrument_id: &str,
) -> std::path::PathBuf {
    let all_paths = collect_paths(spool_root);
    all_paths
        .iter()
        .find(|path| {
            path.extension().and_then(|ext| ext.to_str()) == Some("feather")
                && path
                    .parent()
                    .and_then(|parent| parent.file_name())
                    .and_then(|name| name.to_str())
                    == Some(instrument_id)
                && path
                    .parent()
                    .and_then(|parent| parent.parent())
                    .and_then(|parent| parent.file_name())
                    .and_then(|name| name.to_str())
                    == Some(type_dir)
        })
        .cloned()
        .unwrap_or_else(|| {
            panic!("expected feather file at {type_dir}/{instrument_id}; spool tree: {all_paths:?}")
        })
}

fn assert_schema_instrument_id(file: &std::path::Path, expected_instrument_id: &str) {
    let bytes = std::fs::read(file).unwrap();
    assert!(
        !bytes.is_empty(),
        "feather file should not be empty: {file:?}"
    );
    let reader = StreamReader::try_new(Cursor::new(bytes), None).unwrap();
    let metadata = reader.schema().metadata().clone();
    assert_eq!(
        metadata.get("instrument_id").map(String::as_str),
        Some(expected_instrument_id),
        "expected metadata instrument_id={expected_instrument_id} in {file:?}"
    );
}

fn read_record_batches(file: &std::path::Path) -> Vec<RecordBatch> {
    let bytes = std::fs::read(file).unwrap();
    let reader = StreamReader::try_new(Cursor::new(bytes), None).unwrap();
    reader
        .collect::<Result<Vec<_>, _>>()
        .expect("decode feather record batches")
}

fn read_jsonl_values(path: &std::path::Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn fixed_binary_col(batch: &RecordBatch, name: &str) -> Vec<Vec<u8>> {
    let array = batch
        .column_by_name(name)
        .unwrap_or_else(|| panic!("missing FixedSizeBinary column {name}"))
        .as_any()
        .downcast_ref::<FixedSizeBinaryArray>()
        .unwrap_or_else(|| panic!("column {name} is not FixedSizeBinaryArray"));
    (0..array.len()).map(|i| array.value(i).to_vec()).collect()
}

fn u64_col(batch: &RecordBatch, name: &str) -> Vec<u64> {
    let array = batch
        .column_by_name(name)
        .unwrap_or_else(|| panic!("missing UInt64 column {name}"))
        .as_any()
        .downcast_ref::<UInt64Array>()
        .unwrap_or_else(|| panic!("column {name} is not UInt64Array"));
    (0..array.len()).map(|i| array.value(i)).collect()
}

fn u8_col(batch: &RecordBatch, name: &str) -> Vec<u8> {
    let array = batch
        .column_by_name(name)
        .unwrap_or_else(|| panic!("missing UInt8 column {name}"))
        .as_any()
        .downcast_ref::<UInt8Array>()
        .unwrap_or_else(|| panic!("column {name} is not UInt8Array"));
    (0..array.len()).map(|i| array.value(i)).collect()
}

fn u32_col(batch: &RecordBatch, name: &str) -> Vec<u32> {
    let array = batch
        .column_by_name(name)
        .unwrap_or_else(|| panic!("missing UInt32 column {name}"))
        .as_any()
        .downcast_ref::<UInt32Array>()
        .unwrap_or_else(|| panic!("column {name} is not UInt32Array"));
    (0..array.len()).map(|i| array.value(i)).collect()
}

fn str_col(batch: &RecordBatch, name: &str) -> Vec<String> {
    let array = batch
        .column_by_name(name)
        .unwrap_or_else(|| panic!("missing Utf8 column {name}"))
        .as_any()
        .downcast_ref::<StringArray>()
        .unwrap_or_else(|| panic!("column {name} is not StringArray"));
    (0..array.len())
        .map(|i| array.value(i).to_string())
        .collect()
}

#[test]
fn builds_live_instance_spool_path() {
    let root = spool_root_for_instance("var/normalized", "instance-123");

    assert_eq!(root, "var/normalized/live/instance-123");
}

#[tokio::test(flavor = "current_thread")]
async fn rejects_non_local_catalog_paths() {
    let _guard = live_node_test_lock().lock().await;
    let local = LocalSet::new();

    local
        .run_until(async {
            let node = LiveNode::builder(TraderId::from("TESTER-001"), Environment::Live)
                .unwrap()
                .build()
                .unwrap();

            let result = bolt_v2::nt_runtime_capture::wire_nt_runtime_capture(
                &node,
                node.handle(),
                "s3://bucket/catalog",
                1000,
                50,
                None,
            );

            assert!(result.is_err());
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn accepts_valid_contract_path_on_capture_startup() {
    let _guard = live_node_test_lock().lock().await;
    let local = LocalSet::new();

    local
        .run_until(async {
            let dir = tempdir().unwrap();
            let catalog_root = dir.path().join("catalog");
            let node = LiveNode::builder(TraderId::from("TESTER-001"), Environment::Live)
                .unwrap()
                .build()
                .unwrap();

            let guards = bolt_v2::nt_runtime_capture::wire_nt_runtime_capture(
                &node,
                node.handle(),
                catalog_root.to_str().unwrap(),
                1000,
                50,
                Some(repo_path("contracts/polymarket.toml").to_str().unwrap()),
            )
            .unwrap();

            guards.shutdown().await.unwrap();
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn rejects_missing_contract_path_on_capture_startup() {
    let _guard = live_node_test_lock().lock().await;
    let local = LocalSet::new();

    local
        .run_until(async {
            let dir = tempdir().unwrap();
            let catalog_root = dir.path().join("catalog");
            let node = LiveNode::builder(TraderId::from("TESTER-001"), Environment::Live)
                .unwrap()
                .build()
                .unwrap();
            let missing = dir.path().join("missing-contract.toml");

            let err = bolt_v2::nt_runtime_capture::wire_nt_runtime_capture(
                &node,
                node.handle(),
                catalog_root.to_str().unwrap(),
                1000,
                50,
                Some(missing.to_str().unwrap()),
            )
            .err()
            .expect("missing contract path should fail");

            assert!(err.to_string().contains("failed to read contract"), "{err}");
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn rejects_invalid_contract_path_on_capture_startup() {
    let _guard = live_node_test_lock().lock().await;
    let local = LocalSet::new();

    local
        .run_until(async {
            let dir = tempdir().unwrap();
            let catalog_root = dir.path().join("catalog");
            let node = LiveNode::builder(TraderId::from("TESTER-001"), Environment::Live)
                .unwrap()
                .build()
                .unwrap();
            let invalid = dir.path().join("invalid-contract.toml");
            std::fs::write(&invalid, "not [valid toml").unwrap();

            let err = bolt_v2::nt_runtime_capture::wire_nt_runtime_capture(
                &node,
                node.handle(),
                catalog_root.to_str().unwrap(),
                1000,
                50,
                Some(invalid.to_str().unwrap()),
            )
            .err()
            .expect("invalid contract path should fail");

            assert!(
                err.to_string().contains("failed to parse contract"),
                "{err}"
            );
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn captures_broad_nt_runtime_jsonl_records_outside_hot_path() {
    let _guard = live_node_test_lock().lock().await;
    let local = LocalSet::new();

    local
        .run_until(async {
            let dir = tempdir().unwrap();
            let catalog_root = dir.path().join("catalog");

            let mut node = LiveNode::builder(TraderId::from("TESTER-001"), Environment::Live)
                .unwrap()
                .build()
                .unwrap();
            let handle = node.handle();
            let instance_id = node.instance_id().to_string();
            let guards = bolt_v2::nt_runtime_capture::wire_nt_runtime_capture(
                &node,
                handle.clone(),
                catalog_root.to_str().unwrap(),
                60_000,
                50,
                None,
            )
            .unwrap();

            let instrument_id = InstrumentId::from("0xbroad-123456789.POLYMARKET");
            let publisher_handle = handle.clone();
            tokio::task::spawn_local(async move {
                while !publisher_handle.is_running() {
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }

                let status = InstrumentStatus::new(
                    instrument_id,
                    MarketStatusAction::Pause,
                    2.into(),
                    2.into(),
                    Some("halted by venue".into()),
                    None,
                    Some(false),
                    None,
                    None,
                );
                publish_any(
                    switchboard::get_instrument_status_topic(instrument_id),
                    &status,
                );

                let account_state = AccountState::new(
                    AccountId::from("POLYMARKET-001"),
                    AccountType::Betting,
                    vec![],
                    vec![],
                    true,
                    UUID4::default(),
                    3.into(),
                    3.into(),
                    Some(Currency::USD()),
                );
                publish_account_state("events.account.POLYMARKET-001".into(), &account_state);

                let funding = FundingRateUpdate::new(
                    instrument_id,
                    "0.0001".parse().unwrap(),
                    Some(60),
                    Some(4.into()),
                    4.into(),
                    4.into(),
                );
                publish_funding_rate(switchboard::get_funding_rate_topic(instrument_id), &funding);

                publisher_handle.stop();
            });

            node.run().await.unwrap();
            guards.shutdown().await.unwrap();

            let spool_root = catalog_root.join("live").join(instance_id);
            let status_rows =
                read_jsonl_values(&spool_root.join("status").join("instrument_status.jsonl"));
            assert_eq!(status_rows.len(), 1);
            let status_row = &status_rows[0];
            assert_eq!(status_row["instrument_id"], "0xbroad-123456789.POLYMARKET");
            assert_eq!(status_row["action"], "PAUSE");
            assert_eq!(status_row["reason"], "halted by venue");
            assert_eq!(status_row["is_trading"], false);
            assert_eq!(status_row["ts_event"], 2);
            assert_eq!(status_row["ts_init"], 2);

            let account_rows =
                read_jsonl_values(&spool_root.join("accounts").join("account_state.jsonl"));
            assert_eq!(account_rows.len(), 1);
            let account_row = &account_rows[0];
            assert_eq!(account_row["account_id"], "POLYMARKET-001");
            assert_eq!(account_row["account_type"], "BETTING");
            assert_eq!(account_row["base_currency"], "USD");
            assert_eq!(account_row["is_reported"], true);
            assert_eq!(account_row["ts_event"], 3);
            assert_eq!(account_row["ts_init"], 3);

            let funding_rows =
                read_jsonl_values(&spool_root.join("funding_rates").join("updates.jsonl"));
            assert_eq!(funding_rows.len(), 1);
            let funding_row = &funding_rows[0];
            assert_eq!(funding_row["instrument_id"], "0xbroad-123456789.POLYMARKET");
            assert_eq!(funding_row["rate"], "0.0001");
            assert_eq!(funding_row["interval"], 60);
            assert_eq!(funding_row["next_funding_ns"], 4);
            assert_eq!(funding_row["ts_event"], 4);
            assert_eq!(funding_row["ts_init"], 4);
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn captures_typed_quote_and_close_status_and_flushes_on_shutdown() {
    let _guard = live_node_test_lock().lock().await;
    let local = LocalSet::new();

    local
        .run_until(async {
            let dir = tempdir().unwrap();
            let catalog_root = dir.path().join("catalog");

            let mut node = LiveNode::builder(TraderId::from("TESTER-001"), Environment::Live)
                .unwrap()
                .build()
                .unwrap();
            let handle = node.handle();
            let instance_id = node.instance_id().to_string();
            let guards = bolt_v2::nt_runtime_capture::wire_nt_runtime_capture(
                &node,
                handle.clone(),
                catalog_root.to_str().unwrap(),
                60_000,
                50,
                None,
            )
            .unwrap();

            let instrument_id = InstrumentId::from("0xabc-123456789.POLYMARKET");
            let publisher_handle = handle.clone();
            tokio::task::spawn_local(async move {
                while !publisher_handle.is_running() {
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }

                let quote = QuoteTick::new(
                    instrument_id,
                    Price::from("0.45"),
                    Price::from("0.55"),
                    Quantity::from("100"),
                    Quantity::from("100"),
                    1.into(),
                    1.into(),
                );
                publish_quote(switchboard::get_quotes_topic(instrument_id), &quote);

                let status = InstrumentStatus::new(
                    instrument_id,
                    MarketStatusAction::Close,
                    2.into(),
                    2.into(),
                    None,
                    None,
                    Some(false),
                    None,
                    None,
                );
                publish_any(
                    switchboard::get_instrument_status_topic(instrument_id),
                    &status,
                );

                publisher_handle.stop();
            });

            node.run().await.unwrap();
            guards.shutdown().await.unwrap();

            let spool_root = catalog_root.join("live").join(&instance_id);
            let status_path = spool_root.join("status").join("instrument_status.jsonl");
            let all_paths = collect_paths(&spool_root);

            let quote_files: Vec<_> = all_paths
                .iter()
                .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("feather"))
                .filter(|path| {
                    path.parent()
                        .and_then(|parent| parent.parent())
                        .and_then(|parent| parent.file_name())
                        .and_then(|name| name.to_str())
                        == Some("quotes")
                })
                .collect();
            assert!(!quote_files.is_empty(), "spool tree: {all_paths:?}");

            let status_text = std::fs::read_to_string(status_path).unwrap();
            assert!(status_text.contains("0xabc-123456789.POLYMARKET"));
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn captures_execution_state_jsonl_records_for_order_and_position_events() {
    let _guard = live_node_test_lock().lock().await;
    let local = LocalSet::new();

    local
        .run_until(async {
            let dir = tempdir().unwrap();
            let catalog_root = dir.path().join("catalog");

            let mut node = LiveNode::builder(TraderId::from("TESTER-001"), Environment::Live)
                .unwrap()
                .build()
                .unwrap();
            let handle = node.handle();
            let instance_id = node.instance_id().to_string();
            let guards = bolt_v2::nt_runtime_capture::wire_nt_runtime_capture(
                &node,
                handle.clone(),
                catalog_root.to_str().unwrap(),
                60_000,
                50,
                None,
            )
            .unwrap();

            let instrument_id = InstrumentId::from("0xabc-123456789.POLYMARKET");
            let strategy_id = StrategyId::from("S-EXEC-001");
            let account_id = AccountId::from("SIM-001");
            let publisher_handle = handle.clone();
            tokio::task::spawn_local(async move {
                while !publisher_handle.is_running() {
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }

                let order_event = OrderEventAny::Submitted(OrderSubmitted::new(
                    TraderId::from("TESTER-001"),
                    strategy_id,
                    instrument_id,
                    ClientOrderId::from("O-001"),
                    account_id,
                    UUID4::default(),
                    11.into(),
                    12.into(),
                ));
                publish_order_event(
                    switchboard::get_event_orders_topic(strategy_id),
                    &order_event,
                );

                let fill_event = OrderEventAny::Filled(OrderFilled::new(
                    TraderId::from("TESTER-001"),
                    strategy_id,
                    instrument_id,
                    ClientOrderId::from("O-002"),
                    VenueOrderId::from("V-002"),
                    account_id,
                    TradeId::from("T-002"),
                    OrderSide::Buy,
                    OrderType::Market,
                    Quantity::from("5"),
                    Price::from("0.52"),
                    Currency::USD(),
                    LiquiditySide::Taker,
                    UUID4::default(),
                    13.into(),
                    14.into(),
                    false,
                    Some(PositionId::from("P-001")),
                    Some(Money::new(0.01, Currency::USD())),
                ));
                publish_order_event(
                    switchboard::get_event_orders_topic(strategy_id),
                    &fill_event,
                );

                let position_event = PositionEvent::PositionOpened(PositionOpened {
                    trader_id: TraderId::from("TESTER-001"),
                    strategy_id,
                    instrument_id,
                    position_id: PositionId::from("P-001"),
                    account_id,
                    opening_order_id: ClientOrderId::from("O-001"),
                    entry: nautilus_model::enums::OrderSide::Buy,
                    side: nautilus_model::enums::PositionSide::Long,
                    signed_qty: 10.0,
                    quantity: Quantity::from("10"),
                    last_qty: Quantity::from("10"),
                    last_px: Price::from("0.51"),
                    currency: Currency::USD(),
                    avg_px_open: 0.51,
                    event_id: UUID4::default(),
                    ts_event: 21.into(),
                    ts_init: 22.into(),
                });
                publish_position_event(
                    switchboard::get_event_positions_topic(strategy_id),
                    &position_event,
                );

                let adjusted_event = PositionEvent::PositionAdjusted(PositionAdjusted::new(
                    TraderId::from("TESTER-001"),
                    strategy_id,
                    instrument_id,
                    PositionId::from("P-001"),
                    account_id,
                    PositionAdjustmentType::Commission,
                    Some("1.5".parse().unwrap()),
                    Some(Money::new(-0.02, Currency::USD())),
                    None,
                    UUID4::default(),
                    23.into(),
                    24.into(),
                ));
                publish_position_event(
                    switchboard::get_event_positions_topic(strategy_id),
                    &adjusted_event,
                );

                publisher_handle.stop();
            });

            node.run().await.unwrap();
            guards.shutdown().await.unwrap();

            let spool_root = catalog_root.join("live").join(instance_id);
            let order_events_path = spool_root.join("order_events").join("events.jsonl");
            let position_events_path = spool_root.join("position_events").join("events.jsonl");

            let order_rows: Vec<OrderEventRow> = std::fs::read_to_string(&order_events_path)
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect();
            assert_eq!(order_rows.len(), 2, "{order_rows:?}");
            assert_eq!(order_rows[0].event_type, "Submitted");
            assert_eq!(order_rows[0].client_order_id, "O-001");
            assert_eq!(order_rows[1].event_type, "Filled");
            assert_eq!(order_rows[1].venue_order_id.as_deref(), Some("V-002"));

            let position_rows: Vec<PositionEventRow> =
                std::fs::read_to_string(&position_events_path)
                    .unwrap()
                    .lines()
                    .map(|line| serde_json::from_str(line).unwrap())
                    .collect();
            assert_eq!(position_rows.len(), 2, "{position_rows:?}");
            assert_eq!(position_rows[0].event_type, "PositionOpened");
            assert_eq!(position_rows[0].position_id, "P-001");
            let opened_payload: serde_json::Value =
                serde_json::from_str(&position_rows[0].payload_json).unwrap();
            assert_eq!(opened_payload["trader_id"], "TESTER-001");
            assert_eq!(position_rows[1].event_type, "PositionAdjusted");
            assert_eq!(position_rows[1].realized_pnl.as_deref(), Some("-0.02 USD"));
            let adjusted_payload: serde_json::Value =
                serde_json::from_str(&position_rows[1].payload_json).unwrap();
            assert_eq!(adjusted_payload["trader_id"], "TESTER-001");
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn writes_quote_spool_with_per_instrument_layout_and_metadata() {
    let _guard = live_node_test_lock().lock().await;
    let local = LocalSet::new();

    local
        .run_until(async {
            let dir = tempdir().unwrap();
            let catalog_root = dir.path().join("catalog");

            let mut node = LiveNode::builder(TraderId::from("TESTER-001"), Environment::Live)
                .unwrap()
                .build()
                .unwrap();
            let handle = node.handle();
            let instance_id = node.instance_id().to_string();
            let guards = bolt_v2::nt_runtime_capture::wire_nt_runtime_capture(
                &node,
                handle.clone(),
                catalog_root.to_str().unwrap(),
                60_000,
                50,
                None,
            )
            .unwrap();

            let instrument_id = InstrumentId::from("0xabc-123456789.POLYMARKET");
            let publisher_handle = handle.clone();
            tokio::task::spawn_local(async move {
                while !publisher_handle.is_running() {
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }

                let quote = QuoteTick::new(
                    instrument_id,
                    Price::from("0.45"),
                    Price::from("0.55"),
                    Quantity::from("100"),
                    Quantity::from("100"),
                    1.into(),
                    1.into(),
                );
                publish_quote(switchboard::get_quotes_topic(instrument_id), &quote);
                publisher_handle.stop();
            });

            node.run().await.unwrap();
            guards.shutdown().await.unwrap();

            let spool_root = catalog_root.join("live").join(instance_id);
            let all_paths = collect_paths(&spool_root);
            let quote_file = all_paths
                .iter()
                .find(|path| {
                    path.extension().and_then(|ext| ext.to_str()) == Some("feather")
                        && path
                            .parent()
                            .and_then(|parent| parent.parent())
                            .and_then(|parent| parent.file_name())
                            .and_then(|name| name.to_str())
                            == Some("quotes")
                })
                .expect("quote spool file should exist");

            assert_eq!(
                quote_file
                    .parent()
                    .and_then(|parent| parent.file_name())
                    .and_then(|name| name.to_str()),
                Some("0xabc-123456789.POLYMARKET"),
                "spool tree: {all_paths:?}"
            );

            let bytes = std::fs::read(quote_file).unwrap();
            let reader = StreamReader::try_new(Cursor::new(bytes), None).unwrap();
            let metadata = reader.schema().metadata().clone();

            assert_eq!(
                metadata.get("instrument_id").map(String::as_str),
                Some("0xabc-123456789.POLYMARKET")
            );

            let batches = read_record_batches(quote_file);
            assert_eq!(batches.iter().map(|b| b.num_rows()).sum::<usize>(), 1);
            let batch = &batches[0];
            let schema = batch.schema();
            let column_names: Vec<&str> =
                schema.fields().iter().map(|f| f.name().as_str()).collect();
            assert_eq!(
                column_names,
                vec![
                    "bid_price",
                    "ask_price",
                    "bid_size",
                    "ask_size",
                    "ts_event",
                    "ts_init"
                ],
            );
            assert_eq!(
                fixed_binary_col(batch, "bid_price"),
                vec![Price::from("0.45").raw.to_le_bytes().to_vec()],
            );
            assert_eq!(
                fixed_binary_col(batch, "ask_price"),
                vec![Price::from("0.55").raw.to_le_bytes().to_vec()],
            );
            assert_eq!(
                fixed_binary_col(batch, "bid_size"),
                vec![Quantity::from("100").raw.to_le_bytes().to_vec()],
            );
            assert_eq!(
                fixed_binary_col(batch, "ask_size"),
                vec![Quantity::from("100").raw.to_le_bytes().to_vec()],
            );
            assert_eq!(u64_col(batch, "ts_event"), vec![1u64]);
            assert_eq!(u64_col(batch, "ts_init"), vec![1u64]);
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn does_not_capture_bars_to_flat_spool() {
    let _guard = live_node_test_lock().lock().await;
    let local = LocalSet::new();

    local
        .run_until(async {
            let dir = tempdir().unwrap();
            let catalog_root = dir.path().join("catalog");

            let mut node = LiveNode::builder(TraderId::from("TESTER-001"), Environment::Live)
                .unwrap()
                .build()
                .unwrap();
            let handle = node.handle();
            let instance_id = node.instance_id().to_string();
            let guards = bolt_v2::nt_runtime_capture::wire_nt_runtime_capture(
                &node,
                handle.clone(),
                catalog_root.to_str().unwrap(),
                60_000,
                50,
                None,
            )
            .unwrap();

            let instrument_id = InstrumentId::from("0xabc-123456789.POLYMARKET");
            let bar_type = BarType::new(
                instrument_id,
                nautilus_model::data::bar::BarSpecification::new(
                    1,
                    BarAggregation::Minute,
                    PriceType::Last,
                ),
                nautilus_model::enums::AggregationSource::Internal,
            );
            let publisher_handle = handle.clone();
            tokio::task::spawn_local(async move {
                while !publisher_handle.is_running() {
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }

                let bar = Bar::new(
                    bar_type,
                    Price::from("0.40"),
                    Price::from("0.55"),
                    Price::from("0.35"),
                    Price::from("0.50"),
                    Quantity::from("100"),
                    1.into(),
                    1.into(),
                );
                publish_bar(switchboard::get_bars_topic(bar.bar_type), &bar);
                publisher_handle.stop();
            });

            node.run().await.unwrap();
            guards.shutdown().await.unwrap();

            let spool_root = catalog_root.join("live").join(instance_id);
            let all_paths = collect_paths(&spool_root);
            assert!(
                all_paths.iter().all(|path| {
                    !path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .is_some_and(|name| name.starts_with("bars_"))
                }),
                "bar capture must not create flat spool files: {all_paths:?}"
            );
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn does_not_persist_startup_buffer_if_running_was_never_reached() {
    let _guard = live_node_test_lock().lock().await;
    let local = LocalSet::new();

    local
        .run_until(async {
            let dir = tempdir().unwrap();
            let catalog_root = dir.path().join("catalog");

            let node = LiveNode::builder(TraderId::from("TESTER-001"), Environment::Live)
                .unwrap()
                .build()
                .unwrap();
            let instance_id = node.instance_id().to_string();
            let guards = bolt_v2::nt_runtime_capture::wire_nt_runtime_capture(
                &node,
                node.handle(),
                catalog_root.to_str().unwrap(),
                60_000,
                50,
                None,
            )
            .unwrap();

            let instrument_id = InstrumentId::from("0xstartup-123456789.POLYMARKET");
            let quote = QuoteTick::new(
                instrument_id,
                Price::from("0.45"),
                Price::from("0.55"),
                Quantity::from("100"),
                Quantity::from("100"),
                1.into(),
                1.into(),
            );
            publish_quote(switchboard::get_quotes_topic(instrument_id), &quote);

            let status = InstrumentStatus::new(
                instrument_id,
                MarketStatusAction::Close,
                2.into(),
                2.into(),
                None,
                None,
                Some(false),
                None,
                None,
            );
            publish_any(
                switchboard::get_instrument_status_topic(instrument_id),
                &status,
            );

            guards.shutdown().await.unwrap();

            let spool_root = catalog_root.join("live").join(instance_id);
            let all_paths = collect_paths(&spool_root);
            let feather_files: Vec<_> = all_paths
                .iter()
                .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("feather"))
                .collect();
            let status_path = spool_root.join("status").join("instrument_status.jsonl");

            assert!(feather_files.is_empty(), "spool tree: {all_paths:?}");
            assert!(!status_path.exists(), "spool tree: {all_paths:?}");
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn captures_trading_state_changed_to_risk_jsonl_record() {
    let _guard = live_node_test_lock().lock().await;
    let local = LocalSet::new();

    local
        .run_until(async {
            let dir = tempdir().unwrap();
            let catalog_root = dir.path().join("catalog");

            let mut node = LiveNode::builder(TraderId::from("TESTER-001"), Environment::Live)
                .unwrap()
                .build()
                .unwrap();
            let handle = node.handle();
            let instance_id = node.instance_id().to_string();
            let guards = bolt_v2::nt_runtime_capture::wire_nt_runtime_capture(
                &node,
                handle.clone(),
                catalog_root.to_str().unwrap(),
                60_000,
                50,
                None,
            )
            .unwrap();

            let publisher_handle = handle.clone();
            tokio::task::spawn_local(async move {
                while !publisher_handle.is_running() {
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }

                let event = TradingStateChanged::new(
                    TraderId::from("TESTER-001"),
                    TradingState::Halted,
                    Default::default(),
                    UUID4::default(),
                    7.into(),
                    8.into(),
                );
                publish_any("events.risk".into(), &event);

                publisher_handle.stop();
            });

            node.run().await.unwrap();
            guards.shutdown().await.unwrap();

            let spool_root = catalog_root.join("live").join(instance_id);
            let risk_path = spool_root.join("risk").join("trading_state_changed.jsonl");
            let risk_text = std::fs::read_to_string(&risk_path).unwrap();
            let lines: Vec<&str> = risk_text.lines().collect();
            assert_eq!(lines.len(), 1, "{risk_text}");

            let row: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
            assert_eq!(row["type"], "TradingStateChanged");
            assert_eq!(row["trader_id"], "TESTER-001");
            assert_eq!(row["state"], "HALTED");
            assert_eq!(row["ts_event"], 7);
            assert_eq!(row["ts_init"], 8);
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn captures_trade_tick_to_per_instrument_feather_spool() {
    let _guard = live_node_test_lock().lock().await;
    let local = LocalSet::new();

    local
        .run_until(async {
            let dir = tempdir().unwrap();
            let catalog_root = dir.path().join("catalog");

            let mut node = LiveNode::builder(TraderId::from("TESTER-001"), Environment::Live)
                .unwrap()
                .build()
                .unwrap();
            let handle = node.handle();
            let instance_id = node.instance_id().to_string();
            let guards = bolt_v2::nt_runtime_capture::wire_nt_runtime_capture(
                &node,
                handle.clone(),
                catalog_root.to_str().unwrap(),
                60_000,
                50,
                None,
            )
            .unwrap();

            let instrument_id = InstrumentId::from("0xtrade-123456789.POLYMARKET");
            let publisher_handle = handle.clone();
            tokio::task::spawn_local(async move {
                while !publisher_handle.is_running() {
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }

                let trade = TradeTick::new(
                    instrument_id,
                    Price::from("0.50"),
                    Quantity::from("100"),
                    AggressorSide::Buyer,
                    TradeId::from("T-T01"),
                    1.into(),
                    1.into(),
                );
                publish_trade(switchboard::get_trades_topic(instrument_id), &trade);

                publisher_handle.stop();
            });

            node.run().await.unwrap();
            guards.shutdown().await.unwrap();

            let spool_root = catalog_root.join("live").join(instance_id);
            let trade_file = find_per_instrument_feather_file(
                &spool_root,
                "trades",
                "0xtrade-123456789.POLYMARKET",
            );
            assert_schema_instrument_id(&trade_file, "0xtrade-123456789.POLYMARKET");

            let batches = read_record_batches(&trade_file);
            assert_eq!(batches.iter().map(|b| b.num_rows()).sum::<usize>(), 1);
            let batch = &batches[0];
            assert_eq!(
                fixed_binary_col(batch, "price"),
                vec![Price::from("0.50").raw.to_le_bytes().to_vec()],
            );
            assert_eq!(
                fixed_binary_col(batch, "size"),
                vec![Quantity::from("100").raw.to_le_bytes().to_vec()],
            );
            assert_eq!(str_col(batch, "trade_id"), vec!["T-T01".to_string()]);
            // AggressorSide::Buyer = 1
            assert_eq!(u8_col(batch, "aggressor_side"), vec![1u8]);
            assert_eq!(u64_col(batch, "ts_event"), vec![1u64]);
            assert_eq!(u64_col(batch, "ts_init"), vec![1u64]);
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn captures_order_book_deltas_to_per_instrument_feather_spool() {
    let _guard = live_node_test_lock().lock().await;
    let local = LocalSet::new();

    local
        .run_until(async {
            let dir = tempdir().unwrap();
            let catalog_root = dir.path().join("catalog");

            let mut node = LiveNode::builder(TraderId::from("TESTER-001"), Environment::Live)
                .unwrap()
                .build()
                .unwrap();
            let handle = node.handle();
            let instance_id = node.instance_id().to_string();
            let guards = bolt_v2::nt_runtime_capture::wire_nt_runtime_capture(
                &node,
                handle.clone(),
                catalog_root.to_str().unwrap(),
                60_000,
                50,
                None,
            )
            .unwrap();

            let instrument_id = InstrumentId::from("0xdeltas-123456789.POLYMARKET");
            let publisher_handle = handle.clone();
            tokio::task::spawn_local(async move {
                while !publisher_handle.is_running() {
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }

                let bid = BookOrder::new(
                    OrderSide::Buy,
                    Price::from("0.49"),
                    Quantity::from("100"),
                    1,
                );
                let ask = BookOrder::new(
                    OrderSide::Sell,
                    Price::from("0.51"),
                    Quantity::from("100"),
                    2,
                );
                let bid_delta = OrderBookDelta::new(
                    instrument_id,
                    BookAction::Add,
                    bid,
                    0,
                    1,
                    1.into(),
                    1.into(),
                );
                let ask_delta = OrderBookDelta::new(
                    instrument_id,
                    BookAction::Add,
                    ask,
                    0,
                    2,
                    2.into(),
                    2.into(),
                );
                let deltas = OrderBookDeltas::new(instrument_id, vec![bid_delta, ask_delta]);
                publish_deltas(switchboard::get_book_deltas_topic(instrument_id), &deltas);

                publisher_handle.stop();
            });

            node.run().await.unwrap();
            guards.shutdown().await.unwrap();

            let spool_root = catalog_root.join("live").join(instance_id);
            let deltas_file = find_per_instrument_feather_file(
                &spool_root,
                "order_book_deltas",
                "0xdeltas-123456789.POLYMARKET",
            );
            assert_schema_instrument_id(&deltas_file, "0xdeltas-123456789.POLYMARKET");

            // Each FeatherWriter::write() produces its own RecordBatch (no aggregation
            // across calls), so the two deltas land in two single-row batches.
            let batches = read_record_batches(&deltas_file);
            assert_eq!(batches.len(), 2);
            assert_eq!(batches.iter().map(|b| b.num_rows()).sum::<usize>(), 2);

            // Bid delta — first write
            let bid_batch = &batches[0];
            // BookAction::Add = 1, OrderSide::Buy = 1
            assert_eq!(u8_col(bid_batch, "action"), vec![1u8]);
            assert_eq!(u8_col(bid_batch, "side"), vec![1u8]);
            assert_eq!(
                fixed_binary_col(bid_batch, "price"),
                vec![Price::from("0.49").raw.to_le_bytes().to_vec()],
            );
            assert_eq!(
                fixed_binary_col(bid_batch, "size"),
                vec![Quantity::from("100").raw.to_le_bytes().to_vec()],
            );
            assert_eq!(u64_col(bid_batch, "order_id"), vec![1u64]);
            assert_eq!(u64_col(bid_batch, "sequence"), vec![1u64]);
            assert_eq!(u64_col(bid_batch, "ts_event"), vec![1u64]);
            assert_eq!(u64_col(bid_batch, "ts_init"), vec![1u64]);

            // Ask delta — second write
            let ask_batch = &batches[1];
            // OrderSide::Sell = 2
            assert_eq!(u8_col(ask_batch, "action"), vec![1u8]);
            assert_eq!(u8_col(ask_batch, "side"), vec![2u8]);
            assert_eq!(
                fixed_binary_col(ask_batch, "price"),
                vec![Price::from("0.51").raw.to_le_bytes().to_vec()],
            );
            assert_eq!(
                fixed_binary_col(ask_batch, "size"),
                vec![Quantity::from("100").raw.to_le_bytes().to_vec()],
            );
            assert_eq!(u64_col(ask_batch, "order_id"), vec![2u64]);
            assert_eq!(u64_col(ask_batch, "sequence"), vec![2u64]);
            assert_eq!(u64_col(ask_batch, "ts_event"), vec![2u64]);
            assert_eq!(u64_col(ask_batch, "ts_init"), vec![2u64]);
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn captures_order_book_depth10_to_per_instrument_feather_spool() {
    let _guard = live_node_test_lock().lock().await;
    let local = LocalSet::new();

    local
        .run_until(async {
            let dir = tempdir().unwrap();
            let catalog_root = dir.path().join("catalog");

            let mut node = LiveNode::builder(TraderId::from("TESTER-001"), Environment::Live)
                .unwrap()
                .build()
                .unwrap();
            let handle = node.handle();
            let instance_id = node.instance_id().to_string();
            let guards = bolt_v2::nt_runtime_capture::wire_nt_runtime_capture(
                &node,
                handle.clone(),
                catalog_root.to_str().unwrap(),
                60_000,
                50,
                None,
            )
            .unwrap();

            let instrument_id = InstrumentId::from("0xdepth-123456789.POLYMARKET");
            let publisher_handle = handle.clone();
            tokio::task::spawn_local(async move {
                while !publisher_handle.is_running() {
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }

                let bids = std::array::from_fn(|level| {
                    BookOrder::new(
                        OrderSide::Buy,
                        Price::from(format!("0.4{level}").as_str()),
                        Quantity::from((100 + level).to_string().as_str()),
                        level as u64 + 1,
                    )
                });
                let asks = std::array::from_fn(|level| {
                    BookOrder::new(
                        OrderSide::Sell,
                        Price::from(format!("0.5{level}").as_str()),
                        Quantity::from((200 + level).to_string().as_str()),
                        level as u64 + 11,
                    )
                });
                let bid_counts = std::array::from_fn(|level| level as u32 + 1);
                let ask_counts = std::array::from_fn(|level| level as u32 + 11);
                let depth = OrderBookDepth10::new(
                    instrument_id,
                    bids,
                    asks,
                    bid_counts,
                    ask_counts,
                    0,
                    1,
                    1.into(),
                    1.into(),
                );
                publish_depth10(switchboard::get_book_depth10_topic(instrument_id), &depth);

                publisher_handle.stop();
            });

            node.run().await.unwrap();
            guards.shutdown().await.unwrap();

            let spool_root = catalog_root.join("live").join(instance_id);
            let depth_file = find_per_instrument_feather_file(
                &spool_root,
                "order_book_depths",
                "0xdepth-123456789.POLYMARKET",
            );
            assert_schema_instrument_id(&depth_file, "0xdepth-123456789.POLYMARKET");

            let batches = read_record_batches(&depth_file);
            assert_eq!(batches.iter().map(|b| b.num_rows()).sum::<usize>(), 1);
            let batch = &batches[0];
            let schema = batch.schema();
            let column_names: Vec<&str> =
                schema.fields().iter().map(|f| f.name().as_str()).collect();
            // Depth10 expands each side/level into its own column (0..9).
            assert!(column_names.contains(&"bid_price_0"));
            assert!(column_names.contains(&"bid_price_9"));
            assert!(column_names.contains(&"ask_size_0"));
            assert!(column_names.contains(&"ask_count_9"));
            for level in 0..10 {
                assert_eq!(
                    fixed_binary_col(batch, &format!("bid_price_{level}")),
                    vec![
                        Price::from(format!("0.4{level}").as_str())
                            .raw
                            .to_le_bytes()
                            .to_vec()
                    ],
                );
                assert_eq!(
                    fixed_binary_col(batch, &format!("bid_size_{level}")),
                    vec![
                        Quantity::from((100 + level).to_string().as_str())
                            .raw
                            .to_le_bytes()
                            .to_vec()
                    ],
                );
                assert_eq!(
                    fixed_binary_col(batch, &format!("ask_price_{level}")),
                    vec![
                        Price::from(format!("0.5{level}").as_str())
                            .raw
                            .to_le_bytes()
                            .to_vec()
                    ],
                );
                assert_eq!(
                    fixed_binary_col(batch, &format!("ask_size_{level}")),
                    vec![
                        Quantity::from((200 + level).to_string().as_str())
                            .raw
                            .to_le_bytes()
                            .to_vec()
                    ],
                );
                assert_eq!(
                    u32_col(batch, &format!("bid_count_{level}")),
                    vec![level as u32 + 1],
                );
                assert_eq!(
                    u32_col(batch, &format!("ask_count_{level}")),
                    vec![level as u32 + 11],
                );
            }
            assert_eq!(u8_col(batch, "flags"), vec![0u8]);
            assert_eq!(u64_col(batch, "sequence"), vec![1u64]);
            assert_eq!(u64_col(batch, "ts_event"), vec![1u64]);
            assert_eq!(u64_col(batch, "ts_init"), vec![1u64]);
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn captures_mark_price_update_to_per_instrument_feather_spool() {
    let _guard = live_node_test_lock().lock().await;
    let local = LocalSet::new();

    local
        .run_until(async {
            let dir = tempdir().unwrap();
            let catalog_root = dir.path().join("catalog");

            let mut node = LiveNode::builder(TraderId::from("TESTER-001"), Environment::Live)
                .unwrap()
                .build()
                .unwrap();
            let handle = node.handle();
            let instance_id = node.instance_id().to_string();
            let guards = bolt_v2::nt_runtime_capture::wire_nt_runtime_capture(
                &node,
                handle.clone(),
                catalog_root.to_str().unwrap(),
                60_000,
                50,
                None,
            )
            .unwrap();

            let instrument_id = InstrumentId::from("0xmark-123456789.POLYMARKET");
            let publisher_handle = handle.clone();
            tokio::task::spawn_local(async move {
                while !publisher_handle.is_running() {
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }

                let mark =
                    MarkPriceUpdate::new(instrument_id, Price::from("0.50"), 1.into(), 1.into());
                publish_mark_price(switchboard::get_mark_price_topic(instrument_id), &mark);

                publisher_handle.stop();
            });

            node.run().await.unwrap();
            guards.shutdown().await.unwrap();

            let spool_root = catalog_root.join("live").join(instance_id);
            let mark_file = find_per_instrument_feather_file(
                &spool_root,
                "mark_prices",
                "0xmark-123456789.POLYMARKET",
            );
            assert_schema_instrument_id(&mark_file, "0xmark-123456789.POLYMARKET");

            let batches = read_record_batches(&mark_file);
            assert_eq!(batches.iter().map(|b| b.num_rows()).sum::<usize>(), 1);
            let batch = &batches[0];
            let schema = batch.schema();
            let column_names: Vec<&str> =
                schema.fields().iter().map(|f| f.name().as_str()).collect();
            assert_eq!(column_names, vec!["value", "ts_event", "ts_init"]);
            assert_eq!(
                fixed_binary_col(batch, "value"),
                vec![Price::from("0.50").raw.to_le_bytes().to_vec()],
            );
            assert_eq!(u64_col(batch, "ts_event"), vec![1u64]);
            assert_eq!(u64_col(batch, "ts_init"), vec![1u64]);
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn captures_index_price_update_to_per_instrument_feather_spool() {
    let _guard = live_node_test_lock().lock().await;
    let local = LocalSet::new();

    local
        .run_until(async {
            let dir = tempdir().unwrap();
            let catalog_root = dir.path().join("catalog");

            let mut node = LiveNode::builder(TraderId::from("TESTER-001"), Environment::Live)
                .unwrap()
                .build()
                .unwrap();
            let handle = node.handle();
            let instance_id = node.instance_id().to_string();
            let guards = bolt_v2::nt_runtime_capture::wire_nt_runtime_capture(
                &node,
                handle.clone(),
                catalog_root.to_str().unwrap(),
                60_000,
                50,
                None,
            )
            .unwrap();

            let instrument_id = InstrumentId::from("0xindex-123456789.POLYMARKET");
            let publisher_handle = handle.clone();
            tokio::task::spawn_local(async move {
                while !publisher_handle.is_running() {
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }

                let index =
                    IndexPriceUpdate::new(instrument_id, Price::from("0.50"), 1.into(), 1.into());
                publish_index_price(switchboard::get_index_price_topic(instrument_id), &index);

                publisher_handle.stop();
            });

            node.run().await.unwrap();
            guards.shutdown().await.unwrap();

            let spool_root = catalog_root.join("live").join(instance_id);
            let index_file = find_per_instrument_feather_file(
                &spool_root,
                "index_prices",
                "0xindex-123456789.POLYMARKET",
            );
            assert_schema_instrument_id(&index_file, "0xindex-123456789.POLYMARKET");

            let batches = read_record_batches(&index_file);
            assert_eq!(batches.iter().map(|b| b.num_rows()).sum::<usize>(), 1);
            let batch = &batches[0];
            let schema = batch.schema();
            let column_names: Vec<&str> =
                schema.fields().iter().map(|f| f.name().as_str()).collect();
            assert_eq!(column_names, vec!["value", "ts_event", "ts_init"]);
            assert_eq!(
                fixed_binary_col(batch, "value"),
                vec![Price::from("0.50").raw.to_le_bytes().to_vec()],
            );
            assert_eq!(u64_col(batch, "ts_event"), vec![1u64]);
            assert_eq!(u64_col(batch, "ts_init"), vec![1u64]);
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn captures_instrument_any_to_per_instrument_feather_spool() {
    let _guard = live_node_test_lock().lock().await;
    let local = LocalSet::new();

    local
        .run_until(async {
            let dir = tempdir().unwrap();
            let catalog_root = dir.path().join("catalog");

            let mut node = LiveNode::builder(TraderId::from("TESTER-001"), Environment::Live)
                .unwrap()
                .build()
                .unwrap();
            let handle = node.handle();
            let instance_id = node.instance_id().to_string();
            let guards = bolt_v2::nt_runtime_capture::wire_nt_runtime_capture(
                &node,
                handle.clone(),
                catalog_root.to_str().unwrap(),
                60_000,
                50,
                None,
            )
            .unwrap();

            let instrument_id = InstrumentId::from("0xinstr-123456789.POLYMARKET");
            let publisher_handle = handle.clone();
            tokio::task::spawn_local(async move {
                while !publisher_handle.is_running() {
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }

                let binary = BinaryOption::new(
                    instrument_id,
                    Symbol::from("0xinstr"),
                    AssetClass::Alternative,
                    Currency::USDC(),
                    0.into(),
                    0.into(),
                    3,
                    2,
                    Price::from("0.001"),
                    Quantity::from("0.01"),
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    1.into(),
                    1.into(),
                );
                let instrument = InstrumentAny::BinaryOption(binary);
                publish_any(
                    switchboard::get_instrument_topic(instrument_id),
                    &instrument,
                );

                publisher_handle.stop();
            });

            node.run().await.unwrap();
            guards.shutdown().await.unwrap();

            let spool_root = catalog_root.join("live").join(instance_id);
            let instrument_file = find_per_instrument_feather_file(
                &spool_root,
                "instruments",
                "0xinstr-123456789.POLYMARKET",
            );
            assert_schema_instrument_id(&instrument_file, "0xinstr-123456789.POLYMARKET");

            let batches = read_record_batches(&instrument_file);
            assert_eq!(batches.iter().map(|b| b.num_rows()).sum::<usize>(), 1);
            let batch = &batches[0];
            assert_eq!(
                str_col(batch, "id"),
                vec!["0xinstr-123456789.POLYMARKET".to_string()]
            );
            assert_eq!(str_col(batch, "raw_symbol"), vec!["0xinstr".to_string()]);
            // BinaryOption encoder maps AssetClass::Alternative to the literal "Alternative"
            // (CamelCase), not the strum SCREAMING_SNAKE_CASE form.
            assert_eq!(
                str_col(batch, "asset_class"),
                vec!["Alternative".to_string()]
            );
            assert_eq!(u8_col(batch, "price_precision"), vec![3u8]);
            assert_eq!(u8_col(batch, "size_precision"), vec![2u8]);
            assert_eq!(u64_col(batch, "ts_event"), vec![1u64]);
            assert_eq!(u64_col(batch, "ts_init"), vec![1u64]);
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn captures_instrument_close_to_per_instrument_feather_spool() {
    let _guard = live_node_test_lock().lock().await;
    let local = LocalSet::new();

    local
        .run_until(async {
            let dir = tempdir().unwrap();
            let catalog_root = dir.path().join("catalog");

            let mut node = LiveNode::builder(TraderId::from("TESTER-001"), Environment::Live)
                .unwrap()
                .build()
                .unwrap();
            let handle = node.handle();
            let instance_id = node.instance_id().to_string();
            let guards = bolt_v2::nt_runtime_capture::wire_nt_runtime_capture(
                &node,
                handle.clone(),
                catalog_root.to_str().unwrap(),
                60_000,
                50,
                None,
            )
            .unwrap();

            let instrument_id = InstrumentId::from("0xclose-123456789.POLYMARKET");
            let publisher_handle = handle.clone();
            tokio::task::spawn_local(async move {
                while !publisher_handle.is_running() {
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }

                let close = InstrumentClose::new(
                    instrument_id,
                    Price::from("0.50"),
                    InstrumentCloseType::EndOfSession,
                    1.into(),
                    1.into(),
                );
                publish_any(
                    switchboard::get_instrument_close_topic(instrument_id),
                    &close,
                );

                publisher_handle.stop();
            });

            node.run().await.unwrap();
            guards.shutdown().await.unwrap();

            let spool_root = catalog_root.join("live").join(instance_id);
            let close_file = find_per_instrument_feather_file(
                &spool_root,
                "instrument_closes",
                "0xclose-123456789.POLYMARKET",
            );
            assert_schema_instrument_id(&close_file, "0xclose-123456789.POLYMARKET");

            let batches = read_record_batches(&close_file);
            assert_eq!(batches.iter().map(|b| b.num_rows()).sum::<usize>(), 1);
            let batch = &batches[0];
            let schema = batch.schema();
            let column_names: Vec<&str> =
                schema.fields().iter().map(|f| f.name().as_str()).collect();
            assert_eq!(
                column_names,
                vec!["close_price", "close_type", "ts_event", "ts_init"],
            );
            assert_eq!(
                fixed_binary_col(batch, "close_price"),
                vec![Price::from("0.50").raw.to_le_bytes().to_vec()],
            );
            // InstrumentCloseType::EndOfSession = 1
            assert_eq!(u8_col(batch, "close_type"), vec![1u8]);
            assert_eq!(u64_col(batch, "ts_event"), vec![1u64]);
            assert_eq!(u64_col(batch, "ts_init"), vec![1u64]);
        })
        .await;
}
