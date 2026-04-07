use bolt_v2::normalized_sink::spool_root_for_instance;
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

            let result =
                bolt_v2::normalized_sink::wire_normalized_sinks(&node, "s3://bucket/catalog", 1000);

            assert!(result.is_err());
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn captures_typed_quote_and_close_status_and_flushes_on_shutdown() {
    let local = LocalSet::new();

    local
        .run_until(async {
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

            let dir = tempdir().unwrap();
            let catalog_root = dir.path().join("catalog");

            let node = LiveNode::builder(TraderId::from("TESTER-001"), Environment::Live)
                .unwrap()
                .build()
                .unwrap();
            let instance_id = node.instance_id().to_string();
            let guards = bolt_v2::normalized_sink::wire_normalized_sinks(
                &node,
                catalog_root.to_str().unwrap(),
                60_000,
            )
            .unwrap();

            let instrument_id = InstrumentId::from("0xabc-123456789.POLYMARKET");
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
            let status_path = spool_root.join("status").join("instrument_status.jsonl");
            let all_paths = collect_paths(&spool_root);

            let quote_files: Vec<_> = all_paths
                .iter()
                .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("feather"))
                .filter(|path| {
                    path.file_name()
                        .and_then(|name| name.to_str())
                        .is_some_and(|name| name.starts_with("quotes"))
                })
                .collect();
            assert!(!quote_files.is_empty(), "spool tree: {all_paths:?}");

            let status_text = std::fs::read_to_string(status_path).unwrap();
            assert!(status_text.contains("0xabc-123456789.POLYMARKET"));
        })
        .await;
}
