use std::fs;

use backtesting_vertical_slice::conversion_boundary::{
    CATALOG_METADATA_FILE, CONVERSION_CHECKPOINT_FILE, CONVERSION_CHECKPOINT_VERSION,
    CONVERSION_GENERATION_PATH_MARKER, CONVERSION_MANIFEST_FILE, CatalogConsumptionEvidence,
    CatalogPublicationReceiptIdentity, ConversionCatalogMetadata, ConversionCheckpoint,
    ConversionCheckpointStage, ConversionFingerprint, ConversionManifest, ConversionOutputState,
    inspect_conversion_output, write_completed_conversion_artifacts, write_conversion_checkpoint,
};

fn fingerprint() -> ConversionFingerprint {
    ConversionFingerprint {
        source_proof_id: "source-proof-bybit-spot-tick-trades".to_string(),
        source_proof_version: 1,
        accepted_object_sha256: "d6af93305f3773d6c00b4f3c13ffaef54a573d62ce5e6a96649b06d82df04598"
            .to_string(),
        control_artifact_path: "source-bindings.toml".to_string(),
        control_artifact_sha256: "0000000000000000000000000000000000000000000000000000000000000000"
            .to_string(),
        converter_identity: "csv-native-trades-to-canonical-trades.v1".to_string(),
        converter_version: "1".to_string(),
        converter_config_hash: "converterconfigabc".to_string(),
        catalog_encoding_hash: "1111111111111111111111111111111111111111111111111111111111111111"
            .to_string(),
        conversion_semantics_sha256:
            "2222222222222222222222222222222222222222222222222222222222222222"
                .to_string(),
    }
}

#[test]
fn conversion_generation_is_the_canonical_fingerprint_hash_and_exact_output_suffix() {
    let fingerprint = fingerprint();
    let generation = fingerprint
        .conversion_generation_sha256()
        .expect("derive canonical conversion generation");
    let canonical =
        backtesting_vertical_slice::reference_artifact::canonical_json_sha256(&fingerprint)
            .expect("hash canonical fingerprint JSON");

    assert_eq!(generation, canonical);
    assert_eq!(generation.len(), 64);
    let wire = serde_json::to_value(&fingerprint).expect("serialize fingerprint wire schema");
    assert!(wire.get("conversion_semantics_sha256").is_some());
    assert!(wire.get("run_spec_semantics_sha256").is_none());
    fingerprint
        .validate_output_prefix_generation(&format!(
            "s3://bolt-parquet/nt-research-analytics/backtests/run{CONVERSION_GENERATION_PATH_MARKER}{generation}"
        ))
        .expect("exact generation suffix");

    for invalid in [
        format!(
            "s3://bolt-parquet/nt-research-analytics/backtests/run{CONVERSION_GENERATION_PATH_MARKER}{generation}/"
        ),
        "s3://bolt-parquet/nt-research-analytics/backtests/run".to_string(),
        format!(
            "s3://bolt-parquet/nt-research-analytics/backtests/run{CONVERSION_GENERATION_PATH_MARKER}{}",
            "f".repeat(64)
        ),
        format!(
            "s3://bolt-parquet/nt-research-analytics/backtests/run{CONVERSION_GENERATION_PATH_MARKER}{}{CONVERSION_GENERATION_PATH_MARKER}{generation}",
            "e".repeat(64)
        ),
    ] {
        fingerprint
            .validate_output_prefix_generation(&invalid)
            .expect_err("non-exact generation suffix must fail closed");
    }

    let mut next_generation = fingerprint.clone();
    next_generation.catalog_encoding_hash = "2".repeat(64);
    assert_ne!(
        generation,
        next_generation
            .conversion_generation_sha256()
            .expect("derive changed generation")
    );

    let mut changed_semantics = fingerprint.clone();
    changed_semantics.conversion_semantics_sha256 = "3".repeat(64);
    assert_ne!(
        generation,
        changed_semantics
            .conversion_generation_sha256()
            .expect("derive changed conversion-semantics generation")
    );
}

fn completed_checkpoint(fingerprint: &ConversionFingerprint) -> ConversionCheckpoint {
    ConversionCheckpoint {
        checkpoint_version: CONVERSION_CHECKPOINT_VERSION.to_string(),
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
fn partial_output_without_checkpoint_is_resumable() {
    let dir = tempfile::TempDir::new().unwrap();
    fs::write(dir.path().join("stale.parquet"), b"stale").unwrap();

    assert_eq!(
        inspect_conversion_output(dir.path(), &fingerprint()).unwrap(),
        ConversionOutputState::ResumeFromCheckpoint {
            stage: ConversionCheckpointStage::Started
        }
    );
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
fn source_control_digest_change_invalidates_completed_conversion_reuse() {
    let dir = tempfile::TempDir::new().unwrap();
    let original = fingerprint();
    let checkpoint = completed_checkpoint(&original);
    let checkpoint_hash = checkpoint.content_hash().unwrap();
    let manifest = completed_manifest(&original, checkpoint_hash);
    let metadata = ConversionCatalogMetadata::from_manifest(
        &manifest,
        manifest.content_hash().unwrap(),
        checkpoint.content_hash().unwrap(),
    );
    write_completed_conversion_artifacts(dir.path(), &manifest, &checkpoint, &metadata).unwrap();

    let mut changed_registry = original;
    changed_registry.control_artifact_sha256 = "f".repeat(64);
    let error = inspect_conversion_output(dir.path(), &changed_registry)
        .expect_err("changed source-control digest must invalidate reuse");

    assert!(
        error.to_string().contains("control_artifact_sha256"),
        "{error:#}"
    );
}

#[test]
fn catalog_encoding_change_invalidates_completed_conversion_reuse() {
    let dir = tempfile::TempDir::new().unwrap();
    let original = fingerprint();
    let checkpoint = completed_checkpoint(&original);
    let checkpoint_hash = checkpoint.content_hash().unwrap();
    let manifest = completed_manifest(&original, checkpoint_hash);
    let metadata = ConversionCatalogMetadata::from_manifest(
        &manifest,
        manifest.content_hash().unwrap(),
        checkpoint.content_hash().unwrap(),
    );
    write_completed_conversion_artifacts(dir.path(), &manifest, &checkpoint, &metadata).unwrap();

    let mut changed_encoding = original;
    changed_encoding.catalog_encoding_hash = "2".repeat(64);
    let error = inspect_conversion_output(dir.path(), &changed_encoding)
        .expect_err("changed catalog encoding must invalidate completed-output reuse");

    assert!(
        error.to_string().contains("catalog_encoding_hash"),
        "{error:#}"
    );
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
fn completed_writer_accepts_identical_retry_and_rejects_metadata_replacement() {
    let dir = tempfile::TempDir::new().unwrap();
    let fingerprint = fingerprint();
    let checkpoint = completed_checkpoint(&fingerprint);
    let checkpoint_hash = checkpoint.content_hash().unwrap();
    let manifest = completed_manifest(&fingerprint, checkpoint_hash.clone());
    let manifest_hash = manifest.content_hash().unwrap();
    let metadata =
        ConversionCatalogMetadata::from_manifest(&manifest, manifest_hash.clone(), checkpoint_hash);
    write_completed_conversion_artifacts(dir.path(), &manifest, &checkpoint, &metadata).unwrap();
    write_completed_conversion_artifacts(dir.path(), &manifest, &checkpoint, &metadata)
        .expect("identical completed retry");

    fs::create_dir(dir.path().join("hydrated-catalog")).unwrap();
    let refreshed = metadata
        .clone()
        .with_catalog_consumption_evidence(CatalogConsumptionEvidence::HydratedPublication {
            local_catalog_root: dir.path().join("hydrated-catalog"),
            receipt: CatalogPublicationReceiptIdentity {
                catalog_root_uri: "s3://durable-catalog/".to_string(),
                receipt_uri: "s3://durable-catalog/publication-receipt.json".to_string(),
                receipt_sha256: "1".repeat(64),
                receipt_version_id: "receipt-version".to_string(),
                physical_manifest_sha256: "2".repeat(64),
            },
        })
        .expect("bind hydrated publication evidence");
    let error =
        write_completed_conversion_artifacts(dir.path(), &manifest, &checkpoint, &refreshed)
            .expect_err("immutable metadata cannot be replaced");
    assert!(
        format!("{error:#}").contains("different bytes"),
        "{error:#}"
    );

    let written_checkpoint: ConversionCheckpoint = serde_json::from_str(
        &fs::read_to_string(dir.path().join(CONVERSION_CHECKPOINT_FILE)).unwrap(),
    )
    .unwrap();
    assert_eq!(written_checkpoint, checkpoint);

    let written_metadata: ConversionCatalogMetadata =
        serde_json::from_str(&fs::read_to_string(dir.path().join(CATALOG_METADATA_FILE)).unwrap())
            .unwrap();
    assert_eq!(written_metadata, metadata);
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
fn partial_failed_run_without_checkpoint_resumes_from_start() {
    let dir = tempfile::TempDir::new().unwrap();
    let fingerprint = fingerprint();
    fs::write(dir.path().join("canonical-trades.parquet"), b"partial").unwrap();

    assert_eq!(
        inspect_conversion_output(dir.path(), &fingerprint).unwrap(),
        ConversionOutputState::ResumeFromCheckpoint {
            stage: ConversionCheckpointStage::Started
        }
    );
}

#[test]
fn noncompleted_checkpoint_cannot_be_persisted() {
    let dir = tempfile::TempDir::new().unwrap();
    let fingerprint = fingerprint();
    let error = write_conversion_checkpoint(
        dir.path(),
        &ConversionCheckpoint::started(fingerprint.clone(), "2026-06-06T00:00:00Z"),
    )
    .expect_err("started checkpoint must stay in memory");
    assert!(
        error
            .to_string()
            .contains("only a completed immutable conversion checkpoint"),
        "{error:#}"
    );
    assert!(!dir.path().join(CONVERSION_CHECKPOINT_FILE).exists());
}

#[test]
fn arbitrary_preterminal_residue_is_reconciled_by_the_operator_retry() {
    let dir = tempfile::TempDir::new().unwrap();
    fs::write(dir.path().join("canonical-trades.parquet"), b"partial").unwrap();

    assert_eq!(
        inspect_conversion_output(dir.path(), &fingerprint()).unwrap(),
        ConversionOutputState::ResumeFromCheckpoint {
            stage: ConversionCheckpointStage::Started
        }
    );
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
    assert!(
        !metadata.catalog_consumption_proven(),
        "fresh metadata must not claim catalog consumption before a runner proves it"
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
