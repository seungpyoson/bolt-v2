use std::{fs, path::Path};

use crate::backtesting_vertical_slice_test_support::{
    generate_evicted_pmxt_object_manifests, tempdir_in_repo_target,
};
use backtesting_vertical_slice::source_archive_index_source_universe::{
    SourceArchiveIndexSourceUniverseCategoryManifest, SourceArchiveIndexSourceUniverseManifest,
    write_source_archive_index_source_universe_manifest_from_spec_file,
};

#[test]
fn source_archive_index_source_universe_manifest_preserves_etag_hash_identity() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let archive_index_path = temp_dir.path().join("source-archive-index-manifest.json");
    let output_dir = temp_dir.path().join("source-universe");
    let category_manifest_path = temp_dir
        .path()
        .join("category-manifests/pmxt-object-manifest-orderbook.json");
    let spec_path = temp_dir
        .path()
        .join("source-archive-index-source-universe.toml");

    fs::write(
        &archive_index_path,
        r#"{
  "schema_version": "source-archive-index-manifest.v1",
  "manifest_id": "source-archive-index-manifest-pmxt-polymarket-v2-current",
  "status": "ready",
  "snapshot_id": "source-archive-index-snapshot-pmxt-polymarket-v2-current-2026-06-10T15",
  "fetched_at_utc": "2026-06-10T16:40:00Z",
  "venue": "pmxt",
  "source": "polymarket-v2-archive",
  "family": "prediction_market_outcome",
  "table_family": "order_book_snapshot_deltas",
  "index_url": "https://archive.pmxt.dev/Polymarket/v2",
  "page_count": 1,
  "object_count": 2,
  "verified_head_count": 2,
  "total_content_length_bytes": 1131035039,
  "first_archive_hour_utc": "2026-06-10T14:00:00Z",
  "last_archive_hour_utc": "2026-06-10T15:00:00Z",
  "artifact_refs": [],
  "records": [
    {
      "page_number": 1,
      "object_label": "polymarket_orderbook_2026-06-10T15",
      "archive_hour_utc": "2026-06-10T15:00:00Z",
      "source_url": "https://r2v2.pmxt.dev/polymarket_orderbook_2026-06-10T15.parquet",
      "listed_size_label": "586780173 bytes",
      "http_status": 200,
      "content_length_bytes": 586780173,
      "last_modified": "Wed, 10 Jun 2026 16:15:53 GMT",
      "etag": "\"9b8839adc79af4b1c8fd607cf5cc8f97-70\""
    },
    {
      "page_number": 1,
      "object_label": "polymarket_orderbook_2026-06-10T14",
      "archive_hour_utc": "2026-06-10T14:00:00Z",
      "source_url": "https://r2v2.pmxt.dev/polymarket_orderbook_2026-06-10T14.parquet",
      "listed_size_label": "544254866 bytes",
      "http_status": 200,
      "content_length_bytes": 544254866,
      "last_modified": "Wed, 10 Jun 2026 15:14:54 GMT",
      "etag": "\"dd4d684ef453fd2780fb11c2a8e0dc7b-65\""
    }
  ]
}"#,
    )
    .expect("write archive index");
    fs::write(
        &spec_path,
        format!(
            r#"
manifest_id = "backfill-source-universe-object-manifest-pmxt-polymarket-v2-current"
universe_id = "backfill-source-universe-pmxt-polymarket-v2-current"
source_archive_index_manifest_path = "{archive_index_path}"
output_dir = "{output_dir}"
category_manifest_path = "{category_manifest_path}"
staging_uri_template = "s3://bolt-parquet/backfill-staging/pmxt/raw/v1/source={{source}}/family={{table_family}}/category={{category}}/dt={{archive_date}}/object={{source_hash}}.parquet"
category = "orderbook"
symbol = "POLYMARKET"
source_binding = "polymarket-parquet-archive-index"
source_hash_algorithm = "r2_multipart_etag"
schema_columns = ["asset_id", "price", "size", "side", "timestamp"]
"#,
            archive_index_path = archive_index_path.display(),
            output_dir = output_dir.display(),
            category_manifest_path = category_manifest_path.display(),
        ),
    )
    .expect("write spec");

    let artifact = write_source_archive_index_source_universe_manifest_from_spec_file(&spec_path)
        .expect("source-universe manifest generation succeeds");
    let manifest: SourceArchiveIndexSourceUniverseManifest =
        serde_json::from_slice(&fs::read(&artifact.path).expect("read manifest"))
            .expect("manifest parses");

    assert_eq!(
        manifest.manifest_id,
        "backfill-source-universe-object-manifest-pmxt-polymarket-v2-current"
    );
    assert_eq!(
        manifest.source_archive_index_manifest_id,
        "source-archive-index-manifest-pmxt-polymarket-v2-current"
    );
    assert_eq!(manifest.object_count, 2);
    assert_eq!(manifest.accepted_bytes, 1_131_035_039);
    assert_eq!(manifest.category_summaries.len(), 1);
    assert_eq!(manifest.category_summaries[0].category, "orderbook");
    assert_eq!(manifest.category_summaries[0].object_count, 2);
    assert_eq!(
        manifest.category_summaries[0].compressed_bytes,
        1_131_035_039
    );
    assert_eq!(manifest.payload_records.len(), 2);

    let first = &manifest.payload_records[0];
    assert_eq!(first.source_hash_algorithm, "r2_multipart_etag");
    assert_eq!(first.source_hash, "\"9b8839adc79af4b1c8fd607cf5cc8f97-70\"");
    assert!(first.sha256.is_none());
    assert!(
        first
            .s3_uri
            .ends_with("object=etag-9b8839adc79af4b1c8fd607cf5cc8f97-70.parquet"),
        "staging URI must use a path-safe ETag identity"
    );

    let category_manifest: SourceArchiveIndexSourceUniverseCategoryManifest =
        serde_json::from_slice(&fs::read(category_manifest_path).expect("read category manifest"))
            .expect("category manifest parses");
    assert_eq!(
        category_manifest.manifest_id,
        "backfill-source-universe-object-manifest-pmxt-polymarket-v2-current-category-orderbook"
    );
    assert_eq!(
        category_manifest.parent_manifest_id,
        "backfill-source-universe-object-manifest-pmxt-polymarket-v2-current"
    );
    assert_eq!(
        category_manifest.source_binding,
        "polymarket-parquet-archive-index"
    );
    assert_eq!(category_manifest.table_family, "order_book_snapshot_deltas");
    assert_eq!(category_manifest.object_count, 2);
    assert_eq!(category_manifest.accepted_bytes, 1_131_035_039);
    assert_eq!(category_manifest.payload_records, manifest.payload_records);
}

#[test]
fn pmxt_source_archive_index_source_universe_reference_manifest_matches_full_index() {
    let reference_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../specs/023-nt-research-analytics-platform/reference");
    let temp_dir = tempdir_in_repo_target();
    let (manifest_path, category_manifest_path) =
        generate_evicted_pmxt_object_manifests(&reference_root, temp_dir.path());
    let manifest: SourceArchiveIndexSourceUniverseManifest =
        serde_json::from_slice(&fs::read(&manifest_path).expect("read manifest"))
            .expect("manifest parses");

    assert_eq!(
        manifest.manifest_id,
        "backfill-source-universe-object-manifest-pmxt-polymarket-v2-current"
    );
    assert_eq!(
        manifest.universe_id,
        "backfill-source-universe-pmxt-polymarket-v2-current"
    );
    assert_eq!(
        manifest.source_archive_index_manifest_id,
        "source-archive-index-manifest-pmxt-polymarket-v2-current"
    );
    assert_eq!(
        manifest.source_archive_index_snapshot_id,
        "source-archive-index-snapshot-pmxt-polymarket-v2-current-2026-06-10T15"
    );
    assert_eq!(manifest.source_hash_algorithm, "r2_multipart_etag");
    assert_eq!(manifest.object_count, 1_351);
    assert_eq!(manifest.accepted_bytes, 557_815_904_970);
    assert_eq!(manifest.category_summaries.len(), 1);
    assert_eq!(manifest.category_summaries[0].category, "orderbook");
    assert_eq!(
        manifest.category_summaries[0].source_binding,
        "polymarket-parquet-archive-index"
    );
    assert_eq!(manifest.category_summaries[0].object_count, 1_351);
    assert_eq!(
        manifest.category_summaries[0].compressed_bytes,
        557_815_904_970
    );
    assert_eq!(manifest.payload_records.len(), 1_351);
    assert!(
        manifest
            .payload_records
            .iter()
            .all(|record| record.sha256.is_none()),
        "PMXT source-universe records must not claim SHA-256 from multipart ETags"
    );

    let first = manifest.payload_records.first().expect("first record");
    assert_eq!(first.archive_date, "2026-06-10T15:00:00Z");
    assert_eq!(first.source_hash, "\"9b8839adc79af4b1c8fd607cf5cc8f97-70\"");
    assert!(
        first
            .s3_uri
            .ends_with("object=etag-9b8839adc79af4b1c8fd607cf5cc8f97-70.parquet")
    );

    let category_manifest: SourceArchiveIndexSourceUniverseCategoryManifest =
        serde_json::from_slice(
            &fs::read(category_manifest_path).expect("read PMXT category manifest"),
        )
        .expect("PMXT category manifest parses");
    assert_eq!(
        category_manifest.manifest_id,
        "backfill-source-universe-object-manifest-pmxt-polymarket-v2-current-category-orderbook"
    );
    assert_eq!(category_manifest.object_count, 1_351);
    assert_eq!(category_manifest.accepted_bytes, 557_815_904_970);
}
