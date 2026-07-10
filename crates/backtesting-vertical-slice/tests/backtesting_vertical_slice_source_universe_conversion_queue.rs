use std::{
    fs,
    path::{Component, Path},
};

use crate::backtesting_vertical_slice_test_support::materialize_evicted_pmxt_object_manifests;
use backtesting_vertical_slice::reference_fixture_index::{
    EvictedFixtureIndex, TIER1_PMXT_CONVERSION_QUEUE_PATH, repo_root_from_manifest_dir,
};
use backtesting_vertical_slice::source_universe_conversion_queue::{
    SourceUniverseConversionQueue, SourceUniverseConversionQueueStatus,
    SourceUniverseConversionWorkState, write_source_universe_conversion_queue_from_spec_file,
};

fn assert_source_manifest_path_is_portable(
    queue: &SourceUniverseConversionQueue,
    expected_manifest_path: &Path,
) {
    assert_eq!(queue.source_manifest_path.as_path(), expected_manifest_path);
    assert!(queue.source_manifest_path.is_relative());
    assert!(
        !queue
            .source_manifest_path
            .components()
            .any(|component| matches!(component, Component::ParentDir)),
        "source_manifest_path must be canonical repo-relative form"
    );

    let manifest_ref = queue
        .artifact_refs
        .iter()
        .find(|artifact_ref| artifact_ref.role == "source_universe_manifest")
        .expect("queue records source-universe manifest artifact ref");
    assert_eq!(manifest_ref.path.as_path(), expected_manifest_path);
    assert!(manifest_ref.path.is_relative());
    assert!(
        !manifest_ref
            .path
            .components()
            .any(|component| matches!(component, Component::ParentDir)),
        "manifest artifact ref path must be canonical repo-relative form"
    );
}

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
    assert_source_manifest_path_is_portable(
        &queue,
        Path::new(
            "specs/023-nt-research-analytics-platform/reference/backfill-source-universe-object-manifests/bybit-public-archive-tick-trades-2025-06-01-2026-06-01/bybit-public-archive-tick-trades-object-manifest.json",
        ),
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

#[test]
fn source_universe_conversion_queue_materializes_every_binance_all_instrument_manifest_object() {
    let reference_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../specs/023-nt-research-analytics-platform/reference");
    let manifest_path = reference_root
        .join("backfill-source-universe-object-manifests/binance-data-vision-trades-2026-03-01-all-instruments/binance-data-vision-trades-object-manifest.json");
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let output_dir = temp_dir.path().join("conversion-queue");
    let spec_path = temp_dir
        .path()
        .join("source-universe-conversion-queue.toml");

    fs::write(
        &spec_path,
        format!(
            r#"
queue_id = "source-universe-conversion-queue-binance-data-vision-trades-2026-03-01-all-instruments"
source_universe_manifest_path = "{manifest_path}"
output_dir = "{output_dir}"
table_family = "trades"
output_prefix_template = "source-universe={universe_id}/category={category}/symbol={symbol}/dt={archive_date}/object={sha256}"
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
        "source-universe-conversion-queue-binance-data-vision-trades-2026-03-01-all-instruments"
    );
    assert_eq!(queue.status, SourceUniverseConversionQueueStatus::Ready);
    assert_eq!(
        queue.manifest_id,
        "backfill-source-universe-object-manifest-binance-data-vision-trades-2026-03-01-all-instruments"
    );
    assert_source_manifest_path_is_portable(
        &queue,
        Path::new(
            "specs/023-nt-research-analytics-platform/reference/backfill-source-universe-object-manifests/binance-data-vision-trades-2026-03-01-all-instruments/binance-data-vision-trades-object-manifest.json",
        ),
    );
    assert_eq!(
        queue.universe_id,
        "backfill-source-universe-binance-data-vision-trades-2026-03-01-all-instruments"
    );
    assert_eq!(queue.family, "native_trades");
    assert_eq!(queue.table_family, "trades");
    assert_eq!(queue.work_item_count, 2_051);
    assert_eq!(queue.total_source_bytes, 1_748_721_970);
    assert_eq!(queue.pending_conversion_items, 2_051);
    assert_eq!(queue.category_summaries.len(), 5);
    assert_eq!(queue.category_summaries[0].category, "spot");
    assert_eq!(queue.category_summaries[0].work_item_count, 1_416);
    assert_eq!(queue.category_summaries[0].source_bytes, 406_812_965);
    assert_eq!(queue.category_summaries[1].category, "usd_m_perpetual");
    assert_eq!(queue.category_summaries[1].work_item_count, 593);
    assert_eq!(queue.category_summaries[1].source_bytes, 1_316_489_815);
    assert_eq!(queue.category_summaries[2].category, "usd_m_delivery");
    assert_eq!(queue.category_summaries[2].work_item_count, 4);
    assert_eq!(queue.category_summaries[2].source_bytes, 891_154);
    assert_eq!(queue.category_summaries[3].category, "coin_m_perpetual");
    assert_eq!(queue.category_summaries[3].work_item_count, 28);
    assert_eq!(queue.category_summaries[3].source_bytes, 23_588_863);
    assert_eq!(queue.category_summaries[4].category, "coin_m_delivery");
    assert_eq!(queue.category_summaries[4].work_item_count, 10);
    assert_eq!(queue.category_summaries[4].source_bytes, 939_173);
    assert!(
        queue
            .work_items
            .iter()
            .all(|item| item.work_state == SourceUniverseConversionWorkState::PendingConversion),
        "every source-universe object must become a pending conversion work item"
    );
    assert!(
        queue
            .work_items
            .iter()
            .all(|item| item.table_family == "trades"),
        "Binance native-trades source objects must convert into canonical trade table-family records"
    );

    let first = queue.work_items.first().expect("first work item");
    assert_eq!(first.category, "spot");
    assert_eq!(first.symbol, "0GTRY");
    assert_eq!(first.archive_date, "2026-03-01");
    assert_eq!(first.source_binding, "binance-spot-native-trades");
    assert_eq!(first.source_bytes, 32_220);
    assert_eq!(
        first.source_sha256,
        "054fa3d832a2e262855e93f46f636e66b1bac17b29caf4cd6e4a597b494422e4"
    );
    assert!(
        first.output_prefix.ends_with(
            "source-universe=backfill-source-universe-binance-data-vision-trades-2026-03-01-all-instruments/category=spot/symbol=0GTRY/dt=2026-03-01/object=054fa3d832a2e262855e93f46f636e66b1bac17b29caf4cd6e4a597b494422e4"
        ),
        "output prefix must be derived from the manifest object, not a symbol-only fixture"
    );
}

#[test]
fn source_universe_conversion_queue_preserves_non_sha_source_hash_without_sha256_claim() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let manifest_path = temp_dir.path().join("pmxt-source-universe.json");
    let output_dir = temp_dir.path().join("conversion-queue");
    let spec_path = temp_dir
        .path()
        .join("source-universe-conversion-queue.toml");

    fs::write(
        &manifest_path,
        r#"{
  "schema_version": "backfill-source-universe-object-manifest.v1",
  "manifest_id": "backfill-source-universe-object-manifest-pmxt-polymarket-v2-current",
  "universe_id": "backfill-source-universe-pmxt-polymarket-v2-current",
  "venue": "pmxt",
  "source": "polymarket-v2-archive",
  "family": "prediction_market_outcome",
  "table_family": "order_book_snapshot_deltas",
  "object_count": 1,
  "accepted_bytes": 586780173,
  "category_summaries": [
    {
      "category": "orderbook",
      "source_binding": "polymarket-parquet-archive-index",
      "instrument_count": 1,
      "object_count": 1,
      "compressed_bytes": 586780173,
      "first_archive_date": "2026-06-10T15:00:00Z",
      "last_archive_date": "2026-06-10T15:00:00Z"
    }
  ],
  "payload_records": [
    {
      "s3_uri": "s3://bolt-parquet/backfill-staging/pmxt/raw/v1/source=polymarket-v2-archive/family=order_book_snapshot_deltas/category=orderbook/dt=2026-06-10T15:00:00Z/object=etag-9b8839adc79af4b1c8fd607cf5cc8f97-70.parquet",
      "source_url": "https://r2v2.pmxt.dev/polymarket_orderbook_2026-06-10T15.parquet",
      "source_hash_algorithm": "r2_multipart_etag",
      "source_hash": "\"9b8839adc79af4b1c8fd607cf5cc8f97-70\"",
      "bytes": 586780173,
      "archive_date": "2026-06-10T15:00:00Z",
      "category": "orderbook",
      "symbol": "POLYMARKET",
      "source_binding": "polymarket-parquet-archive-index",
      "schema_columns": ["asset_id", "price", "size", "side", "timestamp"]
    }
  ]
}"#,
    )
    .expect("write manifest");

    fs::write(
        &spec_path,
        format!(
            r#"
queue_id = "source-universe-conversion-queue-pmxt-polymarket-v2-current"
source_universe_manifest_path = "{manifest_path}"
output_dir = "{output_dir}"
output_prefix_template = "source-universe={{universe_id}}/category={{category}}/symbol={{symbol}}/dt={{archive_date}}/object={{source_hash}}"
"#,
            manifest_path = manifest_path.display(),
            output_dir = output_dir.display(),
        ),
    )
    .expect("write spec");

    let artifact = write_source_universe_conversion_queue_from_spec_file(&spec_path)
        .expect("queue generation succeeds");
    let queue: SourceUniverseConversionQueue =
        serde_json::from_slice(&fs::read(&artifact.path).expect("read queue"))
            .expect("queue parses");
    let first = queue.work_items.first().expect("first work item");

    assert_eq!(first.source_hash_algorithm, "r2_multipart_etag");
    assert_eq!(first.source_hash, "\"9b8839adc79af4b1c8fd607cf5cc8f97-70\"");
    assert_eq!(first.source_sha256, "");
    assert!(
        first
            .work_item_id
            .ends_with(":\"9b8839adc79af4b1c8fd607cf5cc8f97-70\"")
    );
    assert!(
        first
            .output_prefix
            .ends_with("/object=etag-9b8839adc79af4b1c8fd607cf5cc8f97-70"),
        "output prefix must sanitize non-path-safe source hashes"
    );
}

#[test]
fn source_universe_conversion_queue_materializes_every_pmxt_archive_index_object_with_evicted_sha256_claim()
 {
    let reference_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../specs/023-nt-research-analytics-platform/reference");
    materialize_evicted_pmxt_object_manifests(&reference_root);
    let committed_spec_path = reference_root.join(
        "source-universe-conversion-queues/pmxt-polymarket-v2-current/source-universe-conversion-queue.toml",
    );
    let artifact = write_source_universe_conversion_queue_from_spec_file(&committed_spec_path)
        .expect("PMXT queue remains reproducible");
    let evicted_index =
        EvictedFixtureIndex::load(&repo_root_from_manifest_dir()).expect("load eviction index");
    let queue_entry = evicted_index
        .entry_for(TIER1_PMXT_CONVERSION_QUEUE_PATH)
        .unwrap_or_else(|| {
            panic!("evicted fixture index does not contain {TIER1_PMXT_CONVERSION_QUEUE_PATH}")
        });
    assert_eq!(
        (artifact.bytes, artifact.content_hash.clone()),
        (queue_entry.bytes, queue_entry.sha256.clone()),
        "regenerated PMXT conversion queue bytes must match the evicted fixture index"
    );
    let queue: SourceUniverseConversionQueue =
        serde_json::from_slice(&fs::read(&artifact.path).expect("read queue"))
            .expect("queue parses");

    assert_eq!(
        queue.queue_id,
        "source-universe-conversion-queue-pmxt-polymarket-v2-current"
    );
    assert_eq!(queue.status, SourceUniverseConversionQueueStatus::Ready);
    assert_eq!(
        queue.manifest_id,
        "backfill-source-universe-object-manifest-pmxt-polymarket-v2-current"
    );
    assert_eq!(
        queue.universe_id,
        "backfill-source-universe-pmxt-polymarket-v2-current"
    );
    assert_eq!(queue.work_item_count, 1_351);
    assert_eq!(queue.pending_conversion_items, 1_351);
    assert_eq!(queue.total_source_bytes, 557_815_904_970);
    assert_eq!(queue.category_summaries.len(), 1);
    assert_eq!(queue.category_summaries[0].category, "orderbook");
    assert_eq!(
        queue.category_summaries[0].source_binding,
        "polymarket-parquet-archive-index"
    );
    assert_eq!(queue.category_summaries[0].work_item_count, 1_351);
    assert_eq!(queue.category_summaries[0].source_bytes, 557_815_904_970);
    assert!(
        queue
            .work_items
            .iter()
            .all(|item| item.work_state == SourceUniverseConversionWorkState::PendingConversion),
        "every PMXT archive index object must become a pending conversion work item"
    );
    assert!(
        queue
            .work_items
            .iter()
            .all(|item| item.source_sha256.is_empty()),
        "PMXT queue must not claim SHA-256 for multipart ETag source objects"
    );

    let first = queue.work_items.first().expect("first work item");
    assert_eq!(first.category, "orderbook");
    assert_eq!(first.symbol, "POLYMARKET");
    assert_eq!(first.archive_date, "2026-06-10T15:00:00Z");
    assert_eq!(first.source_binding, "polymarket-parquet-archive-index");
    assert_eq!(first.source_hash_algorithm, "r2_multipart_etag");
    assert_eq!(first.source_hash, "\"9b8839adc79af4b1c8fd607cf5cc8f97-70\"");
    assert_eq!(first.source_bytes, 586_780_173);
    assert!(
        first.output_prefix.ends_with(
            "source-universe=backfill-source-universe-pmxt-polymarket-v2-current/category=orderbook/symbol=POLYMARKET/dt=2026-06-10T15:00:00Z/object=etag-9b8839adc79af4b1c8fd607cf5cc8f97-70"
        ),
        "output prefix must be derived from the PMXT archive index object"
    );
}
