use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

use bolt_v2::{
    lake_batch::convert_live_spool_to_parquet,
    normalized_sink,
    venue_contract::{
        Capability, CompletenessReport, Policy, Provenance, StreamContract, VenueContract,
    },
};
use nautilus_common::{
    enums::Environment,
    msgbus::{publish_deltas, publish_mark_price, publish_quote, publish_trade, switchboard},
};
use nautilus_live::node::LiveNode;
use nautilus_model::{
    data::{BookOrder, MarkPriceUpdate, OrderBookDelta, OrderBookDeltas, QuoteTick, TradeTick},
    enums::{AggressorSide, BookAction, OrderSide},
    identifiers::{InstrumentId, TradeId, TraderId},
    types::{Price, Quantity},
};
use tempfile::tempdir;
use tokio::task::LocalSet;

fn test_instrument_id() -> InstrumentId {
    InstrumentId::from("0xTEST.POLYMARKET")
}

fn venue_contract_test_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn base_polymarket_streams() -> BTreeMap<String, StreamContract> {
    let supported = |policy: Policy| StreamContract {
        capability: Capability::Supported,
        policy: Some(policy),
        provenance: Provenance::Native,
        reason: None,
        derived_from: None,
    };
    let unsupported = || StreamContract {
        capability: Capability::Unsupported,
        policy: None,
        provenance: Provenance::Native,
        reason: Some("n/a".to_string()),
        derived_from: None,
    };

    BTreeMap::from([
        ("quotes".to_string(), supported(Policy::Required)),
        ("trades".to_string(), supported(Policy::Required)),
        ("order_book_deltas".to_string(), supported(Policy::Required)),
        ("order_book_depths".to_string(), unsupported()),
        ("index_prices".to_string(), unsupported()),
        ("mark_prices".to_string(), unsupported()),
        ("instrument_closes".to_string(), unsupported()),
    ])
}

fn make_contract(streams: BTreeMap<String, StreamContract>) -> VenueContract {
    VenueContract {
        schema_version: 1,
        venue: "test".to_string(),
        adapter_version: "bolt-v2".to_string(),
        streams,
    }
}

#[test]
fn loads_polymarket_contract() {
    let contract =
        VenueContract::load_and_validate(std::path::Path::new("contracts/polymarket.toml"))
            .expect("polymarket contract should load");

    assert_eq!(contract.venue, "polymarket");
    assert_eq!(contract.schema_version, 1);
    assert_eq!(contract.streams.len(), 7);
}

#[test]
fn rejects_contract_missing_stream_class() {
    let mut streams = base_polymarket_streams();
    streams.remove("quotes");
    let contract = make_contract(streams);
    let err = contract.validate().unwrap_err();
    assert!(
        err.to_string()
            .contains("contract missing required stream class"),
        "unexpected error: {err}"
    );
}

#[test]
fn rejects_unsupported_with_required_policy() {
    let mut streams = base_polymarket_streams();
    streams.get_mut("mark_prices").unwrap().policy = Some(Policy::Required);
    let contract = make_contract(streams);
    let err = contract.validate().unwrap_err();
    assert!(
        err.to_string()
            .contains("unsupported capability cannot have policy"),
        "unexpected error: {err}"
    );
}

#[test]
fn rejects_derived_without_derived_from() {
    let mut streams = base_polymarket_streams();
    streams.get_mut("quotes").unwrap().provenance = Provenance::Derived;
    let contract = make_contract(streams);
    let err = contract.validate().unwrap_err();
    assert!(
        err.to_string()
            .contains("derived provenance requires non-empty derived_from"),
        "unexpected error: {err}"
    );
}

#[test]
fn rejects_wrong_schema_version() {
    let mut contract = make_contract(base_polymarket_streams());
    contract.schema_version = 99;
    let err = contract.validate().unwrap_err();
    assert!(
        err.to_string()
            .contains("unsupported contract schema_version"),
        "unexpected error: {err}"
    );
}

#[test]
fn rejects_malformed_toml() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("bad.toml");
    std::fs::write(&path, "this is not valid toml [[[").unwrap();
    let err = VenueContract::load_and_validate(&path).unwrap_err();
    assert!(
        err.to_string().contains("failed to parse contract"),
        "unexpected error: {err}"
    );
}

#[test]
fn contract_happy_path_polymarket() {
    let _guard = venue_contract_test_lock().lock().unwrap();
    let contract =
        VenueContract::load_and_validate(std::path::Path::new("contracts/polymarket.toml"))
            .unwrap();

    let local = LocalSet::new();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let source_dir = tempdir().unwrap();
    let output_dir = tempdir().unwrap();
    let catalog_root = source_dir.path().join("catalog");
    let inst = test_instrument_id();

    let instance_id = runtime.block_on(local.run_until(async {
        let mut node = LiveNode::builder(TraderId::from("TESTER-001"), Environment::Live)
            .unwrap()
            .build()
            .unwrap();
        let handle = node.handle();
        let instance_id = node.instance_id().to_string();

        let guards = normalized_sink::wire_normalized_sinks(
            &node,
            handle.clone(),
            catalog_root.to_str().unwrap(),
            60_000,
            None,
        )
        .unwrap();

        let publisher_handle = handle.clone();
        tokio::task::spawn_local(async move {
            while !publisher_handle.is_running() {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }

            let ts = 1_000_000_000u64;

            let quote = QuoteTick::new(
                inst,
                Price::from("0.55"),
                Price::from("0.56"),
                Quantity::from("100"),
                Quantity::from("100"),
                ts.into(),
                ts.into(),
            );
            publish_quote(switchboard::get_quotes_topic(inst), &quote);

            let trade = TradeTick {
                instrument_id: inst,
                price: Price::from("0.55"),
                size: Quantity::from("10"),
                aggressor_side: AggressorSide::Buyer,
                trade_id: TradeId::new("T1"),
                ts_event: ts.into(),
                ts_init: ts.into(),
            };
            publish_trade(switchboard::get_trades_topic(inst), &trade);

            let delta = OrderBookDelta::new(
                inst,
                BookAction::Add,
                BookOrder::new(OrderSide::Buy, Price::from("0.54"), Quantity::from("50"), 1),
                0,
                0,
                ts.into(),
                ts.into(),
            );
            let deltas = OrderBookDeltas::new(inst, vec![delta]);
            publish_deltas(switchboard::get_book_deltas_topic(inst), &deltas);

            publisher_handle.stop();
        });

        node.run().await.unwrap();
        guards.shutdown().await.unwrap();
        instance_id
    }));

    let report = convert_live_spool_to_parquet(
        catalog_root.as_path(),
        &instance_id,
        output_dir.path(),
        Some(&contract),
    )
    .unwrap();

    let cr = report.completeness.unwrap();
    assert_eq!(cr.outcome, "pass");
    assert_eq!(cr.classes["quotes"].status, "pass");
    assert_eq!(cr.classes["trades"].status, "pass");
    assert_eq!(cr.classes["order_book_deltas"].status, "pass");
    assert_eq!(cr.classes["order_book_depths"].status, "pass_unsupported");
    assert_eq!(cr.classes["index_prices"].status, "pass_unsupported");
    assert_eq!(cr.classes["mark_prices"].status, "pass_unsupported");
    assert_eq!(cr.classes["instrument_closes"].status, "pass_unsupported");

    let report_path = output_dir.path().join("completeness_report.json");
    assert!(report_path.exists());
    let json_str = std::fs::read_to_string(&report_path).unwrap();
    let from_disk: CompletenessReport = serde_json::from_str(&json_str).unwrap();
    assert_eq!(from_disk.outcome, "pass");
}

#[test]
fn contract_fails_when_required_class_absent() {
    let _guard = venue_contract_test_lock().lock().unwrap();
    let contract =
        VenueContract::load_and_validate(std::path::Path::new("contracts/polymarket.toml"))
            .unwrap();

    let local = LocalSet::new();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let source_dir = tempdir().unwrap();
    let output_dir = tempdir().unwrap();
    let catalog_root = source_dir.path().join("catalog");
    let inst = test_instrument_id();

    let instance_id = runtime.block_on(local.run_until(async {
        let mut node = LiveNode::builder(TraderId::from("TESTER-001"), Environment::Live)
            .unwrap()
            .build()
            .unwrap();
        let handle = node.handle();
        let instance_id = node.instance_id().to_string();

        let guards = normalized_sink::wire_normalized_sinks(
            &node,
            handle.clone(),
            catalog_root.to_str().unwrap(),
            60_000,
            None,
        )
        .unwrap();

        let publisher_handle = handle.clone();
        tokio::task::spawn_local(async move {
            while !publisher_handle.is_running() {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }

            let ts = 1_000_000_000u64;

            let quote = QuoteTick::new(
                inst,
                Price::from("0.55"),
                Price::from("0.56"),
                Quantity::from("100"),
                Quantity::from("100"),
                ts.into(),
                ts.into(),
            );
            publish_quote(switchboard::get_quotes_topic(inst), &quote);
            publisher_handle.stop();
        });

        node.run().await.unwrap();
        guards.shutdown().await.unwrap();
        instance_id
    }));

    let err = convert_live_spool_to_parquet(
        catalog_root.as_path(),
        &instance_id,
        output_dir.path(),
        Some(&contract),
    )
    .unwrap_err();

    let msg = err.to_string();
    assert!(msg.contains("contract validation failed"), "{msg}");
    assert!(msg.contains("fail_required_absent"), "{msg}");
}

#[test]
fn contract_fails_when_unsupported_class_has_data() {
    let _guard = venue_contract_test_lock().lock().unwrap();
    let contract =
        VenueContract::load_and_validate(std::path::Path::new("contracts/polymarket.toml"))
            .unwrap();

    let local = LocalSet::new();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let source_dir = tempdir().unwrap();
    let output_dir = tempdir().unwrap();
    let catalog_root = source_dir.path().join("catalog");
    let inst = test_instrument_id();

    let instance_id = runtime.block_on(local.run_until(async {
        let mut node = LiveNode::builder(TraderId::from("TESTER-001"), Environment::Live)
            .unwrap()
            .build()
            .unwrap();
        let handle = node.handle();
        let instance_id = node.instance_id().to_string();

        let guards = normalized_sink::wire_normalized_sinks(
            &node,
            handle.clone(),
            catalog_root.to_str().unwrap(),
            60_000,
            None,
        )
        .unwrap();

        let publisher_handle = handle.clone();
        tokio::task::spawn_local(async move {
            while !publisher_handle.is_running() {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }

            let ts = 1_000_000_000u64;

            let quote = QuoteTick::new(
                inst,
                Price::from("0.55"),
                Price::from("0.56"),
                Quantity::from("100"),
                Quantity::from("100"),
                ts.into(),
                ts.into(),
            );
            publish_quote(switchboard::get_quotes_topic(inst), &quote);

            let trade = TradeTick {
                instrument_id: inst,
                price: Price::from("0.55"),
                size: Quantity::from("10"),
                aggressor_side: AggressorSide::Buyer,
                trade_id: TradeId::new("T1"),
                ts_event: ts.into(),
                ts_init: ts.into(),
            };
            publish_trade(switchboard::get_trades_topic(inst), &trade);

            let delta = OrderBookDelta::new(
                inst,
                BookAction::Add,
                BookOrder::new(OrderSide::Buy, Price::from("0.54"), Quantity::from("50"), 1),
                0,
                0,
                ts.into(),
                ts.into(),
            );
            publish_deltas(
                switchboard::get_book_deltas_topic(inst),
                &OrderBookDeltas::new(inst, vec![delta]),
            );

            let mark = MarkPriceUpdate::new(inst, Price::from("0.55"), ts.into(), ts.into());
            publish_mark_price(switchboard::get_mark_price_topic(inst), &mark);

            publisher_handle.stop();
        });

        node.run().await.unwrap();
        guards.shutdown().await.unwrap();
        instance_id
    }));

    let err = convert_live_spool_to_parquet(
        catalog_root.as_path(),
        &instance_id,
        output_dir.path(),
        Some(&contract),
    )
    .unwrap_err();

    let msg = err.to_string();
    assert!(msg.contains("contract validation failed"), "{msg}");
    assert!(msg.contains("fail_contract_violation"), "{msg}");
}

#[test]
fn contract_fails_when_unknown_class_has_data() {
    let _guard = venue_contract_test_lock().lock().unwrap();
    let contract =
        VenueContract::load_and_validate(std::path::Path::new("contracts/polymarket.toml"))
            .unwrap();

    let local = LocalSet::new();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let source_dir = tempdir().unwrap();
    let output_dir = tempdir().unwrap();
    let catalog_root = source_dir.path().join("catalog");
    let inst = test_instrument_id();

    let instance_id = runtime.block_on(local.run_until(async {
        let mut node = LiveNode::builder(TraderId::from("TESTER-001"), Environment::Live)
            .unwrap()
            .build()
            .unwrap();
        let handle = node.handle();
        let instance_id = node.instance_id().to_string();

        let guards = normalized_sink::wire_normalized_sinks(
            &node,
            handle.clone(),
            catalog_root.to_str().unwrap(),
            60_000,
            None,
        )
        .unwrap();

        let publisher_handle = handle.clone();
        let catalog_root_clone = catalog_root.clone();
        let instance_id_clone = instance_id.clone();
        tokio::task::spawn_local(async move {
            while !publisher_handle.is_running() {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }

            let ts = 1_000_000_000u64;

            let quote = QuoteTick::new(
                inst,
                Price::from("0.55"),
                Price::from("0.56"),
                Quantity::from("100"),
                Quantity::from("100"),
                ts.into(),
                ts.into(),
            );
            publish_quote(switchboard::get_quotes_topic(inst), &quote);

            let trade = TradeTick {
                instrument_id: inst,
                price: Price::from("0.55"),
                size: Quantity::from("10"),
                aggressor_side: AggressorSide::Buyer,
                trade_id: TradeId::new("T1"),
                ts_event: ts.into(),
                ts_init: ts.into(),
            };
            publish_trade(switchboard::get_trades_topic(inst), &trade);

            let delta = OrderBookDelta::new(
                inst,
                BookAction::Add,
                BookOrder::new(OrderSide::Buy, Price::from("0.54"), Quantity::from("50"), 1),
                0,
                0,
                ts.into(),
                ts.into(),
            );
            publish_deltas(
                switchboard::get_book_deltas_topic(inst),
                &OrderBookDeltas::new(inst, vec![delta]),
            );

            let fake_dir = catalog_root_clone
                .join("live")
                .join(&instance_id_clone)
                .join("funding_rates");
            std::fs::create_dir_all(&fake_dir).unwrap();
            std::fs::write(fake_dir.join("dummy.feather"), b"fake feather content").unwrap();

            publisher_handle.stop();
        });

        node.run().await.unwrap();
        guards.shutdown().await.unwrap();
        instance_id
    }));

    let err = convert_live_spool_to_parquet(
        catalog_root.as_path(),
        &instance_id,
        output_dir.path(),
        Some(&contract),
    )
    .unwrap_err();

    let msg = err.to_string();
    assert!(msg.contains("contract validation failed"), "{msg}");
    assert!(msg.contains("fail_unknown"), "{msg}");
}

#[test]
fn no_contract_mode_behaves_as_before() {
    let _guard = venue_contract_test_lock().lock().unwrap();
    let local = LocalSet::new();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let source_dir = tempdir().unwrap();
    let output_dir = tempdir().unwrap();
    let catalog_root = source_dir.path().join("catalog");
    let inst = test_instrument_id();

    let instance_id = runtime.block_on(local.run_until(async {
        let mut node = LiveNode::builder(TraderId::from("TESTER-001"), Environment::Live)
            .unwrap()
            .build()
            .unwrap();
        let handle = node.handle();
        let instance_id = node.instance_id().to_string();

        let guards = normalized_sink::wire_normalized_sinks(
            &node,
            handle.clone(),
            catalog_root.to_str().unwrap(),
            60_000,
            None,
        )
        .unwrap();

        let publisher_handle = handle.clone();
        tokio::task::spawn_local(async move {
            while !publisher_handle.is_running() {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }

            let ts = 1_000_000_000u64;

            let quote = QuoteTick::new(
                inst,
                Price::from("0.55"),
                Price::from("0.56"),
                Quantity::from("100"),
                Quantity::from("100"),
                ts.into(),
                ts.into(),
            );
            publish_quote(switchboard::get_quotes_topic(inst), &quote);
            publisher_handle.stop();
        });

        node.run().await.unwrap();
        guards.shutdown().await.unwrap();
        instance_id
    }));

    let report = convert_live_spool_to_parquet(
        catalog_root.as_path(),
        &instance_id,
        output_dir.path(),
        None,
    )
    .unwrap();

    assert!(report.completeness.is_none());
    assert!(report.converted_classes.contains(&"quotes"));
}
