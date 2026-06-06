use std::fs;

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
        converter_identity: "bybit-public-archive-spot-tick-trades-to-canonical-trades.v1"
            .to_string(),
        converter_version: "1".to_string(),
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
    ConversionManifest {
        manifest_version: "conversion-manifest.v1".to_string(),
        fingerprint: fingerprint.clone(),
        normalized_schema_version: "market_data.v1".to_string(),
        nt_data_type: "TradeTick".to_string(),
        nt_instrument_id: "BNBUSDC.BYBIT".to_string(),
        canonical_rows: 3,
        output_catalog_uri: "s3://bolt-parquet/nt-research-analytics/backtests/run/nt-catalog"
            .to_string(),
        catalog_hash: "catalog-hash".to_string(),
        checkpoint_hash,
        completed_at: "2026-06-06T00:00:00Z".to_string(),
    }
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
