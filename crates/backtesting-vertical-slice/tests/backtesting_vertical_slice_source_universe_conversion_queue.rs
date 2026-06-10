use std::{fs, path::Path};

use backtesting_vertical_slice::source_universe_conversion_queue::{
    SourceUniverseConversionQueue, SourceUniverseConversionQueueStatus,
    SourceUniverseConversionWorkState, write_source_universe_conversion_queue_from_spec_file,
};

#[test]
fn source_universe_conversion_queue_materializes_every_bybit_manifest_object() {
    let reference_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../specs/023-nt-research-analytics-platform/reference");
    let manifest_path = reference_root
        .join("backfill-source-universe-object-manifests/bybit-public-archive-tick-trades-2025-06-01-2026-06-01/bybit-public-archive-tick-trades-object-manifest.json");
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let output_dir = temp_dir.path().join("conversion-queue");
    let spec_path = temp_dir
        .path()
        .join("source-universe-conversion-queue.toml");

    fs::write(
        &spec_path,
        format!(
            r#"
queue_id = "source-universe-conversion-queue-bybit-public-archive-tick-trades-2025-06-01-2026-06-01"
source_universe_manifest_path = "{manifest_path}"
output_dir = "{output_dir}"
output_prefix_template = "s3://bolt-parquet/nt-research-analytics/backtests/source-universe={universe_id}/category={category}/symbol={symbol}/dt={archive_date}/object={sha256}"
"#,
            manifest_path = manifest_path.display(),
            output_dir = output_dir.display(),
            universe_id = "{universe_id}",
            category = "{category}",
            symbol = "{symbol}",
            archive_date = "{archive_date}",
            sha256 = "{sha256}",
        ),
    )
    .expect("write spec");

    let artifact = write_source_universe_conversion_queue_from_spec_file(&spec_path)
        .expect("queue generation succeeds");
    let queue: SourceUniverseConversionQueue =
        serde_json::from_slice(&fs::read(&artifact.path).expect("read queue"))
            .expect("queue parses");

    assert_eq!(
        queue.queue_id,
        "source-universe-conversion-queue-bybit-public-archive-tick-trades-2025-06-01-2026-06-01"
    );
    assert_eq!(queue.status, SourceUniverseConversionQueueStatus::Ready);
    assert_eq!(
        queue.manifest_id,
        "backfill-source-universe-object-manifest-bybit-public-archive-tick-trades-2025-06-01-2026-06-01"
    );
    assert_eq!(
        queue.universe_id,
        "backfill-source-universe-bybit-public-archive-tick-trades-2025-06-01-2026-06-01"
    );
    assert_eq!(queue.work_item_count, 5_857);
    assert_eq!(queue.total_source_bytes, 20_309_079_098);
    assert_eq!(queue.pending_conversion_items, 5_857);
    assert_eq!(queue.category_summaries.len(), 3);
    assert_eq!(queue.category_summaries[0].category, "inverse");
    assert_eq!(queue.category_summaries[0].work_item_count, 702);
    assert_eq!(queue.category_summaries[1].category, "linear");
    assert_eq!(queue.category_summaries[1].work_item_count, 1_851);
    assert_eq!(queue.category_summaries[2].category, "spot");
    assert_eq!(queue.category_summaries[2].work_item_count, 3_304);
    assert!(
        queue
            .work_items
            .iter()
            .all(|item| item.work_state == SourceUniverseConversionWorkState::PendingConversion),
        "every source-universe object must become a pending conversion work item"
    );

    let first = queue.work_items.first().expect("first work item");
    assert_eq!(first.category, "inverse");
    assert_eq!(first.symbol, "AAVEUSD");
    assert_eq!(first.archive_date, "2025-06-01");
    assert_eq!(first.source_binding, "bybit-inverse-tick-trades");
    assert_eq!(first.source_bytes, 124_717);
    assert_eq!(
        first.source_sha256,
        "0c92b646ffca8f0621eb36741b3d7382c9212905d781905ff066bfc0b5d72516"
    );
    assert!(
        first.output_prefix.ends_with(
            "source-universe=backfill-source-universe-bybit-public-archive-tick-trades-2025-06-01-2026-06-01/category=inverse/symbol=AAVEUSD/dt=2025-06-01/object=0c92b646ffca8f0621eb36741b3d7382c9212905d781905ff066bfc0b5d72516"
        ),
        "output prefix must be derived from the manifest object, not a daily fixture"
    );
}
