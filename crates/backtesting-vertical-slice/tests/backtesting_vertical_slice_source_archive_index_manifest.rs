use std::fs;
use std::path::Path;

use backtesting_vertical_slice::source_archive_index_manifest::{
    SourceArchiveIndexManifest, SourceArchiveIndexManifestStatus,
    write_source_archive_index_manifest_from_spec_file,
};

#[test]
fn source_archive_index_manifest_summarizes_verified_index_snapshot() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let snapshot_path = temp_dir.path().join("archive-index-snapshot.json");
    let output_dir = temp_dir.path().join("manifest");
    let spec_path = temp_dir.path().join("source-archive-index-manifest.toml");

    fs::write(
        &snapshot_path,
        r#"{
  "schema_version": "source-archive-index-snapshot.v1",
  "snapshot_id": "synthetic-archive-index-snapshot",
  "fetched_at_utc": "2026-06-10T16:40:00Z",
  "venue": "pmxt",
  "source": "polymarket-v2-archive",
  "family": "orderbook",
  "table_family": "order_book_snapshot_deltas",
  "index_url": "https://archive.example.test/Polymarket/v2",
  "page_count": 2,
  "records": [
    {
      "page_number": 1,
      "object_label": "polymarket_orderbook_2026-06-10T15",
      "archive_hour_utc": "2026-06-10T15:00:00Z",
      "source_url": "https://r2.example.test/polymarket_orderbook_2026-06-10T15.parquet",
      "listed_size_label": "559.6 MB",
      "http_status": 200,
      "content_length_bytes": 586782310,
      "last_modified": "Wed, 10 Jun 2026 15:06:44 GMT",
      "etag": "\"object-a\""
    },
    {
      "page_number": 1,
      "object_label": "polymarket_orderbook_2026-06-10T14",
      "archive_hour_utc": "2026-06-10T14:00:00Z",
      "source_url": "https://r2.example.test/polymarket_orderbook_2026-06-10T14.parquet",
      "listed_size_label": "519.0 MB",
      "http_status": 200,
      "content_length_bytes": 544210944,
      "last_modified": "Wed, 10 Jun 2026 14:06:44 GMT",
      "etag": "\"object-b\""
    },
    {
      "page_number": 2,
      "object_label": "polymarket_orderbook_2026-06-08T14",
      "archive_hour_utc": "2026-06-08T14:00:00Z",
      "source_url": "https://r2.example.test/polymarket_orderbook_2026-06-08T14.parquet",
      "listed_size_label": "544.4 MB",
      "http_status": 200,
      "content_length_bytes": 570844774,
      "last_modified": "Mon, 08 Jun 2026 14:06:44 GMT",
      "etag": "\"object-c\""
    }
  ]
}"#,
    )
    .expect("write snapshot");
    fs::write(
        &spec_path,
        format!(
            r#"
manifest_id = "source-archive-index-manifest-synthetic"
index_snapshot_path = "{snapshot_path}"
output_dir = "{output_dir}"
"#,
            snapshot_path = snapshot_path.display(),
            output_dir = output_dir.display(),
        ),
    )
    .expect("write spec");

    let first = write_source_archive_index_manifest_from_spec_file(&spec_path).expect("first");
    let second = write_source_archive_index_manifest_from_spec_file(&spec_path).expect("second");
    assert_eq!(first.content_hash, second.content_hash);

    let manifest: SourceArchiveIndexManifest =
        serde_json::from_slice(&fs::read(&first.path).expect("read manifest"))
            .expect("manifest parses");
    assert_eq!(manifest.schema_version, "source-archive-index-manifest.v1");
    assert_eq!(manifest.status, SourceArchiveIndexManifestStatus::Ready);
    assert_eq!(manifest.snapshot_id, "synthetic-archive-index-snapshot");
    assert_eq!(manifest.page_count, 2);
    assert_eq!(manifest.object_count, 3);
    assert_eq!(manifest.verified_head_count, 3);
    assert_eq!(manifest.total_content_length_bytes, 1_701_838_028);
    assert_eq!(manifest.first_archive_hour_utc, "2026-06-08T14:00:00Z");
    assert_eq!(manifest.last_archive_hour_utc, "2026-06-10T15:00:00Z");
    assert_eq!(manifest.records.len(), 3);
}

#[test]
fn pmxt_source_archive_index_reference_manifest_matches_snapshot() {
    let reference_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../specs/023-nt-research-analytics-platform/reference");
    let spec_path = reference_root.join(
        "source-archive-index-manifests/pmxt-polymarket-v2-current/source-archive-index-manifest.toml",
    );

    let artifact = write_source_archive_index_manifest_from_spec_file(&spec_path)
        .expect("PMXT archive index manifest remains reproducible");
    let manifest: SourceArchiveIndexManifest =
        serde_json::from_slice(&fs::read(&artifact.path).expect("read PMXT manifest"))
            .expect("PMXT manifest parses");

    assert_eq!(
        manifest.manifest_id,
        "source-archive-index-manifest-pmxt-polymarket-v2-current"
    );
    assert_eq!(manifest.status, SourceArchiveIndexManifestStatus::Ready);
    assert_eq!(
        manifest.snapshot_id,
        "source-archive-index-snapshot-pmxt-polymarket-v2-current-2026-06-10T15"
    );
    assert_eq!(manifest.page_count, 28);
    assert_eq!(manifest.object_count, 1_351);
    assert_eq!(manifest.verified_head_count, 1_351);
    assert_eq!(manifest.total_content_length_bytes, 557_815_904_970);
    assert_eq!(manifest.first_archive_hour_utc, "2026-04-13T19:00:00Z");
    assert_eq!(manifest.last_archive_hour_utc, "2026-06-10T15:00:00Z");
}

#[test]
fn source_archive_index_manifest_rejects_duplicate_source_urls() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let snapshot_path = temp_dir.path().join("archive-index-snapshot.json");
    let output_dir = temp_dir.path().join("manifest");
    let spec_path = temp_dir.path().join("source-archive-index-manifest.toml");

    fs::write(
        &snapshot_path,
        r#"{
  "schema_version": "source-archive-index-snapshot.v1",
  "snapshot_id": "synthetic-archive-index-snapshot",
  "fetched_at_utc": "2026-06-10T16:40:00Z",
  "venue": "synthetic",
  "source": "archive",
  "family": "objects",
  "table_family": "events",
  "index_url": "https://archive.example.test",
  "page_count": 1,
  "records": [
    {
      "page_number": 1,
      "object_label": "a",
      "archive_hour_utc": "2026-06-10T15:00:00Z",
      "source_url": "https://r2.example.test/a.parquet",
      "listed_size_label": "1.0 MB",
      "http_status": 200,
      "content_length_bytes": 1
    },
    {
      "page_number": 1,
      "object_label": "b",
      "archive_hour_utc": "2026-06-10T16:00:00Z",
      "source_url": "https://r2.example.test/a.parquet",
      "listed_size_label": "1.0 MB",
      "http_status": 200,
      "content_length_bytes": 1
    }
  ]
}"#,
    )
    .expect("write snapshot");
    fs::write(
        &spec_path,
        format!(
            r#"
manifest_id = "source-archive-index-manifest-synthetic"
index_snapshot_path = "{snapshot_path}"
output_dir = "{output_dir}"
"#,
            snapshot_path = snapshot_path.display(),
            output_dir = output_dir.display(),
        ),
    )
    .expect("write spec");

    let err = write_source_archive_index_manifest_from_spec_file(&spec_path)
        .expect_err("duplicate source URL must fail");
    assert!(err.to_string().contains("duplicate source URL"), "{err:#}");
}
