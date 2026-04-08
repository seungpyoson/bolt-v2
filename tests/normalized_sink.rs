use bolt_v2::normalized_sink::spool_root_for_instance;
use arrow::ipc::reader::StreamReader;
use nautilus_common::{
    enums::Environment,
    msgbus::{publish_any, publish_quote, switchboard},
};
use nautilus_live::node::LiveNode;
use nautilus_model::{
    data::{InstrumentStatus, QuoteTick},
    enums::MarketStatusAction,
    identifiers::{InstrumentId, TraderId},
    types::{Price, Quantity},
};
use tempfile::tempdir;
use tokio::task::LocalSet;
use std::io::Cursor;

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

#[test]
fn builds_live_instance_spool_path() {
    let root = spool_root_for_instance("var/normalized", "instance-123");

    assert_eq!(root, "var/normalized/live/instance-123");
}

#[tokio::test(flavor = "current_thread")]
async fn rejects_non_local_catalog_paths() {
    let local = LocalSet::new();

    local
        .run_until(async {
            let node = LiveNode::builder(TraderId::from("TESTER-001"), Environment::Live)
                .unwrap()
                .build()
                .unwrap();

            let result = bolt_v2::normalized_sink::wire_normalized_sinks(
                &node,
                node.handle(),
                "s3://bucket/catalog",
                1000,
            );

            assert!(result.is_err());
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn captures_typed_quote_and_close_status_and_flushes_on_shutdown() {
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
            let guards = bolt_v2::normalized_sink::wire_normalized_sinks(
                &node,
                handle.clone(),
                catalog_root.to_str().unwrap(),
                60_000,
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

            let spool_root = catalog_root.join("live").join(instance_id);
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
async fn writes_quote_spool_with_per_instrument_layout_and_metadata() {
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
            let guards = bolt_v2::normalized_sink::wire_normalized_sinks(
                &node,
                handle.clone(),
                catalog_root.to_str().unwrap(),
                60_000,
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
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn does_not_persist_startup_buffer_if_running_was_never_reached() {
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
            let guards = bolt_v2::normalized_sink::wire_normalized_sinks(
                &node,
                node.handle(),
                catalog_root.to_str().unwrap(),
                60_000,
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
