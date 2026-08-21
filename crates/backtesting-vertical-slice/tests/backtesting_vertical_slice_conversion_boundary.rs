use std::fs;

#[cfg(unix)]
use std::os::unix::fs::symlink;

use backtesting_vertical_slice::conversion_boundary::{
    CATALOG_METADATA_FILE, CONVERSION_CHECKPOINT_FILE, CONVERSION_MANIFEST_FILE,
    ConversionCatalogMetadata, ConversionCheckpoint, ConversionCheckpointStage,
    ConversionFingerprint, ConversionManifest, ConversionOutputState, inspect_conversion_output,
    write_completed_conversion_artifacts, write_conversion_checkpoint,
};

fn fingerprint() -> ConversionFingerprint {
    ConversionFingerprint {
        source_proof_id: "source-proof-bybit-spot-tick-trades".to_string(),
        source_proof_version: 1,
        accepted_object_sha256: "d6af93305f3773d6c00b4f3c13ffaef54a573d62ce5e6a96649b06d82df04598"
            .to_string(),
        converter_identity: "csv-native-trades-to-canonical-trades.v1".to_string(),
        converter_version: "1".to_string(),
        converter_config_hash: "converterconfigabc".to_string(),
    }
}

fn completed_checkpoint(fingerprint: &ConversionFingerprint) -> ConversionCheckpoint {
    ConversionCheckpoint {
        checkpoint_version: "conversion-checkpoint.v1".to_string(),
        fingerprint: fingerprint.clone(),
        stage: ConversionCheckpointStage::Completed,
        canonical_rows: Some(3),
        catalog_hash: Some("catalog-hash".to_string()),
        updated_at: "2026-06-06T00:00:00Z".to_string(),
    }
}

fn completed_manifest(
    fingerprint: &ConversionFingerprint,
    checkpoint_hash: String,
) -> ConversionManifest {
    ConversionManifest::completed(
        fingerprint.clone(),
        "market_data.v1",
        "TradeTick",
        "BNBUSDC.BYBIT",
        3,
        "s3://bolt-parquet/nt-research-analytics/backtests/run/nt-catalog",
        "catalog-hash",
        checkpoint_hash,
        "2026-06-06T00:00:00Z",
    )
}

#[test]
fn dirty_existing_converted_output_without_manifest_or_checkpoint_is_rejected() {
    let dir = tempfile::TempDir::new().unwrap();
    fs::write(dir.path().join("stale.parquet"), b"stale").unwrap();

    let err = inspect_conversion_output(dir.path(), &fingerprint()).unwrap_err();

    assert!(err.to_string().contains("dirty conversion output"), "{err}");
}

#[test]
fn existing_output_with_mismatched_source_or_converter_identity_is_rejected() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut other = fingerprint();
    other.accepted_object_sha256 =
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string();
    let checkpoint = completed_checkpoint(&other);
    let checkpoint_hash = checkpoint.content_hash().unwrap();
    let manifest = completed_manifest(&other, checkpoint_hash);
    let metadata = ConversionCatalogMetadata::from_manifest(
        &manifest,
        manifest.content_hash().unwrap(),
        checkpoint.content_hash().unwrap(),
    );
    write_completed_conversion_artifacts(dir.path(), &manifest, &checkpoint, &metadata).unwrap();

    let err = inspect_conversion_output(dir.path(), &fingerprint()).unwrap_err();

    assert!(err.to_string().contains("accepted_object_sha256"), "{err}");
}

#[test]
fn same_manifest_and_checkpoint_rerun_is_idempotent() {
    let dir = tempfile::TempDir::new().unwrap();
    let fingerprint = fingerprint();
    let checkpoint = completed_checkpoint(&fingerprint);
    let checkpoint_hash = checkpoint.content_hash().unwrap();
    let manifest = completed_manifest(&fingerprint, checkpoint_hash.clone());
    let manifest_hash = manifest.content_hash().unwrap();
    let metadata =
        ConversionCatalogMetadata::from_manifest(&manifest, manifest_hash.clone(), checkpoint_hash);
    write_completed_conversion_artifacts(dir.path(), &manifest, &checkpoint, &metadata).unwrap();

    let first = inspect_conversion_output(dir.path(), &fingerprint).unwrap();
    let second = inspect_conversion_output(dir.path(), &fingerprint).unwrap();

    assert_eq!(first, second);
    assert_eq!(
        first,
        ConversionOutputState::Complete {
            manifest_hash,
            checkpoint_hash: checkpoint.content_hash().unwrap(),
            catalog_hash: "catalog-hash".to_string()
        }
    );
}

#[test]
fn completed_writer_finalizes_started_checkpoint_and_refreshes_metadata() {
    let dir = tempfile::TempDir::new().unwrap();
    let fingerprint = fingerprint();
    write_conversion_checkpoint(
        dir.path(),
        &ConversionCheckpoint::started(fingerprint.clone(), "2026-06-06T00:00:00Z"),
    )
    .unwrap();

    let checkpoint = completed_checkpoint(&fingerprint);
    let checkpoint_hash = checkpoint.content_hash().unwrap();
    let manifest = completed_manifest(&fingerprint, checkpoint_hash.clone());
    let manifest_hash = manifest.content_hash().unwrap();
    let metadata =
        ConversionCatalogMetadata::from_manifest(&manifest, manifest_hash.clone(), checkpoint_hash);
    write_completed_conversion_artifacts(dir.path(), &manifest, &checkpoint, &metadata).unwrap();

    let refreshed = metadata
        .clone()
        .with_execution_catalog_access("s3://durable-catalog", true);
    write_completed_conversion_artifacts(dir.path(), &manifest, &checkpoint, &refreshed).unwrap();

    let written_checkpoint: ConversionCheckpoint = serde_json::from_str(
        &fs::read_to_string(dir.path().join(CONVERSION_CHECKPOINT_FILE)).unwrap(),
    )
    .unwrap();
    assert_eq!(written_checkpoint, checkpoint);

    let written_metadata: ConversionCatalogMetadata =
        serde_json::from_str(&fs::read_to_string(dir.path().join(CATALOG_METADATA_FILE)).unwrap())
            .unwrap();
    assert_eq!(written_metadata, refreshed);
}

#[test]
fn legacy_single_family_manifest_without_catalog_row_map_remains_idempotent() {
    let dir = tempfile::TempDir::new().unwrap();
    let fingerprint = fingerprint();
    let checkpoint = completed_checkpoint(&fingerprint);
    let checkpoint_hash = checkpoint.content_hash().unwrap();
    let mut manifest = completed_manifest(&fingerprint, checkpoint_hash.clone());
    manifest.catalog_nt_data_types.clear();
    manifest.catalog_rows_by_nt_data_type.clear();
    let manifest_hash = manifest.content_hash().unwrap();
    let mut metadata =
        ConversionCatalogMetadata::from_manifest(&manifest, manifest_hash.clone(), checkpoint_hash);
    metadata.catalog_nt_data_types.clear();
    metadata.catalog_rows_by_nt_data_type.clear();
    write_completed_conversion_artifacts(dir.path(), &manifest, &checkpoint, &metadata).unwrap();

    let written_manifest =
        std::fs::read_to_string(dir.path().join(CONVERSION_MANIFEST_FILE)).unwrap();
    assert!(!written_manifest.contains("catalog_nt_data_types"));
    assert!(!written_manifest.contains("catalog_rows_by_nt_data_type"));

    assert_eq!(
        manifest.effective_catalog_rows_by_nt_data_type(),
        std::collections::BTreeMap::from([("TradeTick".to_string(), 3)])
    );
    assert_eq!(
        inspect_conversion_output(dir.path(), &fingerprint).unwrap(),
        ConversionOutputState::Complete {
            manifest_hash,
            checkpoint_hash: checkpoint.content_hash().unwrap(),
            catalog_hash: "catalog-hash".to_string()
        }
    );
}

#[test]
fn partial_failed_run_resumes_only_from_validated_checkpoint() {
    let dir = tempfile::TempDir::new().unwrap();
    let fingerprint = fingerprint();
    let checkpoint = ConversionCheckpoint {
        checkpoint_version: "conversion-checkpoint.v1".to_string(),
        fingerprint: fingerprint.clone(),
        stage: ConversionCheckpointStage::CatalogProjected,
        canonical_rows: Some(3),
        catalog_hash: Some("catalog-hash".to_string()),
        updated_at: "2026-06-06T00:00:00Z".to_string(),
    };
    write_conversion_checkpoint(dir.path(), &checkpoint).unwrap();
    fs::write(dir.path().join("canonical-trades.parquet"), b"partial").unwrap();

    assert_eq!(
        inspect_conversion_output(dir.path(), &fingerprint).unwrap(),
        ConversionOutputState::ResumeFromCheckpoint {
            stage: ConversionCheckpointStage::CatalogProjected
        }
    );

    let invalid_dir = tempfile::TempDir::new().unwrap();
    let mut invalid_checkpoint = checkpoint;
    invalid_checkpoint.fingerprint.converter_identity = "other-converter.v1".to_string();
    write_conversion_checkpoint(invalid_dir.path(), &invalid_checkpoint).unwrap();
    fs::write(
        invalid_dir.path().join("canonical-trades.parquet"),
        b"partial",
    )
    .unwrap();

    let err = inspect_conversion_output(invalid_dir.path(), &fingerprint).unwrap_err();
    assert!(err.to_string().contains("converter_identity"), "{err}");
}

#[test]
fn partial_failed_run_without_valid_checkpoint_refuses_to_continue() {
    let dir = tempfile::TempDir::new().unwrap();
    fs::write(dir.path().join("canonical-trades.parquet"), b"partial").unwrap();

    let err = inspect_conversion_output(dir.path(), &fingerprint()).unwrap_err();

    assert!(err.to_string().contains("dirty conversion output"), "{err}");
}

#[test]
fn missing_and_empty_output_roots_are_clean_new_but_empty_subdirectories_are_dirty() {
    let parent = tempfile::TempDir::new().unwrap();
    let missing = parent.path().join("missing-output");
    assert_eq!(
        inspect_conversion_output(&missing, &fingerprint()).unwrap(),
        ConversionOutputState::CleanNew
    );

    let empty = parent.path().join("empty-output");
    fs::create_dir(&empty).unwrap();
    assert_eq!(
        inspect_conversion_output(&empty, &fingerprint()).unwrap(),
        ConversionOutputState::CleanNew
    );

    fs::create_dir(empty.join("empty-subdirectory")).unwrap();
    let error = inspect_conversion_output(&empty, &fingerprint()).unwrap_err();
    assert!(
        error.to_string().contains("dirty conversion output"),
        "{error}"
    );
}

#[cfg(unix)]
#[test]
fn symlinked_output_roots_are_dirty_without_following_the_target() {
    let parent = tempfile::TempDir::new().unwrap();
    let target = parent.path().join("outside-output");
    fs::create_dir(&target).unwrap();
    let sentinel = target.join("sentinel");
    fs::write(&sentinel, b"outside").unwrap();

    let output = parent.path().join("output-link");
    symlink(&target, &output).unwrap();

    let error = inspect_conversion_output(&output, &fingerprint()).unwrap_err();
    assert!(
        error.to_string().contains("not a real directory"),
        "{error}"
    );
    assert_eq!(fs::read(&sentinel).unwrap(), b"outside");
}

#[cfg(unix)]
#[test]
fn dangling_symlink_output_root_is_dirty_instead_of_clean_new() {
    let parent = tempfile::TempDir::new().unwrap();
    let output = parent.path().join("dangling-output-link");
    symlink(parent.path().join("missing-target"), &output).unwrap();

    let error = inspect_conversion_output(&output, &fingerprint()).unwrap_err();
    assert!(
        error.to_string().contains("not a real directory"),
        "{error}"
    );
}

#[cfg(unix)]
#[test]
fn completed_output_with_symlink_descendant_is_rejected_before_reuse() {
    let output = tempfile::TempDir::new().unwrap();
    let checkpoint = completed_checkpoint(&fingerprint());
    let checkpoint_hash = checkpoint.content_hash().unwrap();
    let manifest = completed_manifest(&fingerprint(), checkpoint_hash.clone());
    let metadata = ConversionCatalogMetadata::from_manifest(
        &manifest,
        manifest.content_hash().unwrap(),
        checkpoint_hash,
    );
    write_completed_conversion_artifacts(output.path(), &manifest, &checkpoint, &metadata).unwrap();

    let outside = tempfile::NamedTempFile::new().unwrap();
    fs::write(outside.path(), b"outside").unwrap();
    symlink(outside.path(), output.path().join("linked-artifact")).unwrap();

    let error = inspect_conversion_output(output.path(), &fingerprint()).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("non-regular file linked-artifact"),
        "{error}"
    );
    assert_eq!(fs::read(outside.path()).unwrap(), b"outside");
}

#[test]
fn clean_new_output_writes_manifest_checkpoint_and_catalog_metadata() {
    let dir = tempfile::TempDir::new().unwrap();
    let fingerprint = fingerprint();
    assert_eq!(
        inspect_conversion_output(dir.path(), &fingerprint).unwrap(),
        ConversionOutputState::CleanNew
    );

    let checkpoint = completed_checkpoint(&fingerprint);
    let checkpoint_hash = checkpoint.content_hash().unwrap();
    let manifest = completed_manifest(&fingerprint, checkpoint_hash.clone());
    let manifest_hash = manifest.content_hash().unwrap();
    let metadata =
        ConversionCatalogMetadata::from_manifest(&manifest, manifest_hash.clone(), checkpoint_hash);
    assert_eq!(
        metadata.output_catalog_uri,
        "s3://bolt-parquet/nt-research-analytics/backtests/run/nt-catalog"
    );
    assert_eq!(metadata.execution_catalog_uri, metadata.output_catalog_uri);
    assert!(
        !metadata.direct_s3_catalog_access_proven,
        "fresh metadata must not claim direct S3 catalog access unless a direct S3 BacktestNode run proved it"
    );
    write_completed_conversion_artifacts(dir.path(), &manifest, &checkpoint, &metadata).unwrap();

    assert!(dir.path().join(CONVERSION_MANIFEST_FILE).exists());
    assert!(dir.path().join(CONVERSION_CHECKPOINT_FILE).exists());
    assert!(dir.path().join(CATALOG_METADATA_FILE).exists());

    let state = inspect_conversion_output(dir.path(), &fingerprint).unwrap();
    assert_eq!(
        state,
        ConversionOutputState::Complete {
            manifest_hash,
            checkpoint_hash: checkpoint.content_hash().unwrap(),
            catalog_hash: "catalog-hash".to_string()
        }
    );
}
