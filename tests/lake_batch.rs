use std::path::Path;

#[cfg(unix)]
use std::os::unix::fs::symlink;

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
            publish_any(
                switchboard::get_instrument_close_topic(instrument_id),
                &close,
            );

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
    assert_eq!(
        report.converted_classes,
        vec!["quotes", "instrument_closes"]
    );
    assert!(
        !output_dir.path().join("live").exists(),
        "output tree: {:?}",
        collect_paths(output_dir.path())
    );

    let source_paths = collect_paths(&catalog_root.join("live").join(&instance_id));
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

    let mut catalog = ParquetDataCatalog::new(output_dir.path(), None, None, None, None);
    let quote_files = catalog.get_file_list_from_data_cls("quotes").unwrap();
    assert!(
        !quote_files.is_empty(),
        "quote files: {quote_files:?}; output tree: {:?}",
        collect_paths(output_dir.path())
    );

    let close_files = catalog
        .get_file_list_from_data_cls("instrument_closes")
        .unwrap();
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
fn converts_legacy_flat_spool_layout() {
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

            publisher_handle.stop();
        });

        node.run().await.unwrap();
        guards.shutdown().await.unwrap();
        instance_id
    }));

    // Rearrange per-class dirs into legacy flat layout at instance root.
    // Spool layout is class/<instrument_id>/file.feather — use recursive
    // collect_paths to find feather files at any depth.
    let instance_root = catalog_root.join("live").join(&instance_id);
    let class_dirs: Vec<_> = std::fs::read_dir(&instance_root)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .collect();
    for class_dir in &class_dirs {
        let class_name = class_dir.file_name();
        let class_name = class_name.to_string_lossy();
        let feather_files: Vec<_> = collect_paths(&class_dir.path())
            .into_iter()
            .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("feather"))
            .collect();
        for (i, feather_path) in feather_files.iter().enumerate() {
            let flat_name = format!("{class_name}_{i}.feather");
            std::fs::rename(feather_path, instance_root.join(&flat_name)).unwrap();
        }
        std::fs::remove_dir_all(class_dir.path()).unwrap();
    }

    // Verify no subdirectories remain — purely flat layout.
    let remaining: Vec<_> = std::fs::read_dir(&instance_root)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .collect();
    assert!(
        remaining.is_empty(),
        "expected flat layout, found dirs: {remaining:?}"
    );

    let report =
        convert_live_spool_to_parquet(catalog_root.as_path(), &instance_id, output_dir.path())
            .unwrap();

    assert_eq!(report.converted_classes, vec!["quotes"]);

    let mut catalog = ParquetDataCatalog::new(output_dir.path(), None, None, None, None);
    let quotes = catalog.quote_ticks(None, None, None).unwrap();
    assert_eq!(quotes.len(), 1);
    assert_eq!(quotes[0].instrument_id, instrument_id);
}

#[test]
fn fails_when_output_root_overlaps_catalog_path() {
    let source_root = tempdir().unwrap();
    let instance_dir = source_root.path().join("live").join("instance-123");
    std::fs::create_dir_all(&instance_dir).unwrap();
    let output_root = source_root.path().join("nested-output");

    let result = convert_live_spool_to_parquet(source_root.path(), "instance-123", &output_root);

    let error = result.unwrap_err();
    assert!(
        error
            .to_string()
            .contains("output_root must not overlap catalog_path"),
        "{error:?}"
    );
}

#[test]
fn fails_when_output_root_is_not_empty() {
    let source_root = tempdir().unwrap();
    let instance_dir = source_root.path().join("live").join("instance-123");
    std::fs::create_dir_all(&instance_dir).unwrap();

    let output_root = tempdir().unwrap();
    std::fs::write(output_root.path().join("sentinel.txt"), "existing").unwrap();

    let error =
        convert_live_spool_to_parquet(source_root.path(), "instance-123", output_root.path())
            .unwrap_err();

    assert!(
        error
            .to_string()
            .contains("output_root must be empty before conversion"),
        "{error:?}"
    );
}

#[test]
fn fails_when_instance_id_is_not_a_single_path_segment() {
    let source_root = tempdir().unwrap();
    std::fs::create_dir_all(source_root.path().join("live").join("instance-123")).unwrap();
    let output_root = tempdir().unwrap();

    let error =
        convert_live_spool_to_parquet(source_root.path(), "../..", output_root.path()).unwrap_err();

    assert!(
        error
            .to_string()
            .contains("instance_id must be a single path segment"),
        "{error:?}"
    );
}

#[test]
fn fails_when_no_supported_stream_data_is_present() {
    let source_root = tempdir().unwrap();
    std::fs::create_dir_all(source_root.path().join("live").join("instance-empty")).unwrap();
    let output_root = tempdir().unwrap();

    let error =
        convert_live_spool_to_parquet(source_root.path(), "instance-empty", output_root.path())
            .unwrap_err();

    assert!(
        error
            .to_string()
            .contains("no supported reduced task 4 data found"),
        "{error:?}"
    );
}

#[test]
fn fails_when_only_unsupported_stream_data_is_present() {
    let source_root = tempdir().unwrap();
    let instance_root = source_root.path().join("live").join("instance-bars");
    std::fs::create_dir_all(&instance_root).unwrap();
    std::fs::write(instance_root.join("bars_123.feather"), b"not-used").unwrap();
    let output_root = tempdir().unwrap();

    let error =
        convert_live_spool_to_parquet(source_root.path(), "instance-bars", output_root.path())
            .unwrap_err();

    assert!(
        error
            .to_string()
            .contains("no supported reduced task 4 data found"),
        "{error:?}"
    );
}

#[test]
fn creates_nonexistent_output_root_without_panic() {
    let source_root = tempdir().unwrap();
    let instance_root = source_root.path().join("live").join("instance-fresh");
    std::fs::create_dir_all(instance_root.join("quotes")).unwrap();
    // Empty quotes dir — no feather files, so conversion fails with "no data"
    // rather than panicking on the non-existent output root.
    let output_root = tempdir().unwrap();
    let nonexistent = output_root.path().join("does-not-exist");

    let error = convert_live_spool_to_parquet(source_root.path(), "instance-fresh", &nonexistent)
        .unwrap_err();

    assert!(
        error
            .to_string()
            .contains("no supported reduced task 4 data found"),
        "expected data error, not a panic: {error:?}"
    );
    assert!(nonexistent.is_dir(), "output_root should have been created");
}

#[cfg(unix)]
#[test]
fn skips_symlinks_in_spool_tree() {
    let source_root = tempdir().unwrap();
    let instance_root = source_root.path().join("live").join("instance-sym");
    std::fs::create_dir_all(&instance_root).unwrap();

    // Symlink posing as a supported class directory — must be skipped.
    let external_dir = tempdir().unwrap();
    std::fs::write(external_dir.path().join("quotes_1.feather"), b"external").unwrap();
    symlink(external_dir.path(), instance_root.join("quotes")).unwrap();

    // Symlink to a file at instance root — must be skipped.
    let external_file = tempdir().unwrap();
    let feather_path = external_file.path().join("real.feather");
    std::fs::write(&feather_path, b"external-file").unwrap();
    symlink(&feather_path, instance_root.join("trades_1.feather")).unwrap();

    let output_root = tempdir().unwrap();
    let error =
        convert_live_spool_to_parquet(source_root.path(), "instance-sym", output_root.path())
            .unwrap_err();

    // Both symlinked entries should be skipped, leaving no data to convert.
    assert!(
        error
            .to_string()
            .contains("no supported reduced task 4 data found"),
        "symlinked data should have been skipped: {error:?}"
    );
}
