use std::path::Path;

use bolt_v2::{
    lake_batch::{convert_live_spool_to_parquet, supported_stream_classes},
    normalized_sink,
};
use nautilus_common::{
    enums::Environment,
    msgbus::{publish_any, publish_quote, switchboard},
};
use nautilus_live::node::LiveNode;
use nautilus_model::{
    data::{InstrumentClose, QuoteTick},
    enums::InstrumentCloseType,
    identifiers::{InstrumentId, TraderId},
    types::{Price, Quantity},
};
use nautilus_persistence::backend::catalog::ParquetDataCatalog;
use tempfile::tempdir;
use tokio::task::LocalSet;

fn collect_paths(root: &Path) -> Vec<std::path::PathBuf> {
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
fn exposes_reduced_task_4_supported_stream_classes() {
    assert_eq!(
        supported_stream_classes(),
        &[
            "quotes",
            "trades",
            "order_book_deltas",
            "order_book_depths",
            "index_prices",
            "mark_prices",
            "instrument_closes",
        ]
    );
}

#[test]
fn fails_when_live_spool_instance_is_missing() {
    let source_root = tempdir().unwrap();
    let output_root = tempdir().unwrap();

    let error =
        convert_live_spool_to_parquet(source_root.path(), "missing-instance", output_root.path())
            .unwrap_err();

    assert!(
        error
            .to_string()
            .contains("missing live spool instance directory"),
        "{error:?}"
    );
}

#[test]
fn converts_live_spool_into_queryable_parquet_under_separate_output_root() {
    let local = LocalSet::new();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let source_dir = tempdir().unwrap();
    let output_dir = tempdir().unwrap();
    let catalog_root = source_dir.path().join("catalog");
    let instrument_id = InstrumentId::from("0xabc-123456789.POLYMARKET");

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
        )
        .unwrap();

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

            let close = InstrumentClose::new(
                instrument_id,
                Price::from("0.50"),
                InstrumentCloseType::EndOfSession,
                2.into(),
                2.into(),
            );
            publish_any(switchboard::get_instrument_close_topic(instrument_id), &close);

            publisher_handle.stop();
        });

        node.run().await.unwrap();
        guards.shutdown().await.unwrap();
        instance_id
    }));

    let report =
        convert_live_spool_to_parquet(catalog_root.as_path(), &instance_id, output_dir.path())
            .unwrap();

    assert_eq!(report.instance_id, instance_id);
    assert!(
        output_dir.path().join("live").join(&report.instance_id).exists(),
        "expected staged live spool under output root"
    );

    let source_paths = collect_paths(&catalog_root.join("live").join(&instance_id));
    let staged_paths = collect_paths(&output_dir.path().join("live").join(&report.instance_id));
    assert!(
        source_paths.iter().any(|path| {
            path.extension().and_then(|ext| ext.to_str()) == Some("feather")
                && path
                    .parent()
                    .and_then(|parent| parent.parent())
                    .and_then(|parent| parent.file_name())
                    .and_then(|name| name.to_str())
                    == Some("quotes")
        }),
        "source spool tree: {source_paths:?}"
    );
    assert!(
        staged_paths.iter().any(|path| {
            path.extension().and_then(|ext| ext.to_str()) == Some("feather")
                && path
                    .parent()
                    .and_then(|parent| parent.parent())
                    .and_then(|parent| parent.file_name())
                    .and_then(|name| name.to_str())
                    == Some("quotes")
        }),
        "source spool tree: {source_paths:?}; staged spool tree: {staged_paths:?}"
    );

    let mut catalog = ParquetDataCatalog::new(output_dir.path(), None, None, None, None);
    let quote_files = catalog.get_file_list_from_data_cls("quotes").unwrap();
    assert!(
        !quote_files.is_empty(),
        "quote files: {quote_files:?}; output tree: {:?}",
        collect_paths(output_dir.path())
    );

    let close_files = catalog.get_file_list_from_data_cls("instrument_closes").unwrap();
    assert!(
        !close_files.is_empty(),
        "close files: {close_files:?}; output tree: {:?}",
        collect_paths(output_dir.path())
    );

    let quotes = catalog.quote_ticks(None, None, None).unwrap();
    assert_eq!(quotes.len(), 1);
    assert_eq!(quotes[0].instrument_id, instrument_id);

    let closes = catalog.instrument_closes(None, None, None).unwrap();
    assert_eq!(closes.len(), 1);
    assert_eq!(closes[0].instrument_id, instrument_id);
}

#[test]
fn fails_when_output_root_matches_catalog_path() {
    let source_root = Path::new("var/catalog");

    let result = convert_live_spool_to_parquet(source_root, "instance-123", source_root);

    let error = result.unwrap_err();
    assert!(
        error
            .to_string()
            .contains("output_root must differ from catalog_path"),
        "{error:?}"
    );
}
