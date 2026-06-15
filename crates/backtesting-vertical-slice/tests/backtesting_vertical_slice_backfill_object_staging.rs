use std::{collections::BTreeMap, fs};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use backtesting_vertical_slice::{
    backfill_object_staging::{
        BackfillObjectStagingError, BackfillObjectStagingSpec, stage_backfill_object_with_resolver,
    },
    run_manifest::ManifestArtifactStore,
};
use sha2::{Digest, Sha256};

fn empty_artifact_store() -> ManifestArtifactStore {
    ManifestArtifactStore {
        storage_options: BTreeMap::new(),
        rust_storage_options: BTreeMap::new(),
        ssm_parameters: None,
    }
}

#[test]
fn staging_writes_create_only_object_and_scope_manifest() {
    let dir = tempfile::tempdir().expect("tempdir");
    let local_object = dir.path().join("source.zip");
    let object_bytes = b"trade_id,price\n1,10.00\n";
    fs::write(&local_object, object_bytes).expect("write local object");
    let sha256 = format!("{:x}", Sha256::digest(object_bytes));
    let artifact_root = dir.path().join("artifact-root");
    let output_object = artifact_root.join("raw").join("object.zip");
    let output_object_uri = format!("file://{}", output_object.display());
    let output_dir = dir.path().join("manifest");

    let spec = BackfillObjectStagingSpec {
        staging_id: "single-object-binance-daily".to_string(),
        artifact_root: format!("file://{}", artifact_root.display()),
        artifact_store: empty_artifact_store(),
        local_object_path: local_object,
        output_object_uri: output_object_uri.clone(),
        source_url: "https://data.example.test/data/spot/daily/trades/TEST/TEST.zip".to_string(),
        expected_sha256: sha256.clone(),
        expected_bytes: object_bytes.len() as u64,
        archive_date: "2026-03-01".to_string(),
        schema_columns: vec!["trade_id".to_string(), "price".to_string()],
        output_dir,
    };

    let mut resolver = |_region: &str, _path: &str| {
        Err::<String, String>("no SSM resolution expected".to_string())
    };
    let artifact = stage_backfill_object_with_resolver(&spec, &mut resolver).expect("stage object");

    assert_eq!(
        fs::read(&output_object).expect("read staged object"),
        object_bytes
    );
    assert_eq!(artifact.object_sha256, sha256);
    assert_eq!(artifact.object_bytes, object_bytes.len() as u64);

    let manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(&artifact.manifest_path).expect("read manifest"))
            .expect("parse manifest");
    assert_eq!(manifest["manifest_id"], "single-object-binance-daily");
    assert_eq!(
        manifest["payload_records"]
            .as_array()
            .expect("records")
            .len(),
        1
    );
    assert_eq!(
        manifest["payload_records"][0]["s3_uri"],
        serde_json::Value::String(output_object_uri)
    );
    assert_eq!(
        manifest["payload_records"][0]["schema_columns"],
        serde_json::json!(["trade_id", "price"])
    );
}

#[test]
fn staging_rejects_hash_mismatch_before_write() {
    let dir = tempfile::tempdir().expect("tempdir");
    let local_object = dir.path().join("source.zip");
    fs::write(&local_object, b"payload").expect("write local object");
    let artifact_root = dir.path().join("artifact-root");
    let output_object = artifact_root.join("raw").join("object.zip");
    let spec = BackfillObjectStagingSpec {
        staging_id: "hash-mismatch".to_string(),
        artifact_root: format!("file://{}", artifact_root.display()),
        artifact_store: empty_artifact_store(),
        local_object_path: local_object,
        output_object_uri: format!("file://{}", output_object.display()),
        source_url: "https://data.example.test/object.zip".to_string(),
        expected_sha256: "0000000000000000000000000000000000000000000000000000000000000000"
            .to_string(),
        expected_bytes: 7,
        archive_date: "2026-03-01".to_string(),
        schema_columns: vec!["id".to_string()],
        output_dir: dir.path().join("manifest"),
    };

    let mut resolver = |_region: &str, _path: &str| {
        Err::<String, String>("no SSM resolution expected".to_string())
    };
    let error = stage_backfill_object_with_resolver(&spec, &mut resolver).unwrap_err();

    assert!(matches!(
        error,
        BackfillObjectStagingError::Sha256Mismatch { .. }
    ));
    assert!(!output_object.exists());
}

#[cfg(unix)]
#[test]
fn staging_rejects_byte_mismatch_from_metadata_before_reading_payload() {
    let dir = tempfile::tempdir().expect("tempdir");
    let local_object = dir.path().join("source.zip");
    let object_bytes = b"payload";
    let actual_bytes = object_bytes.len() as u64;
    let expected_bytes = actual_bytes + 1;
    fs::write(&local_object, object_bytes).expect("write local object");
    fs::set_permissions(&local_object, fs::Permissions::from_mode(0o000))
        .expect("make local object unreadable");
    let artifact_root = dir.path().join("artifact-root");
    let output_object = artifact_root.join("raw").join("object.zip");
    let spec = BackfillObjectStagingSpec {
        staging_id: "byte-mismatch".to_string(),
        artifact_root: format!("file://{}", artifact_root.display()),
        artifact_store: empty_artifact_store(),
        local_object_path: local_object,
        output_object_uri: format!("file://{}", output_object.display()),
        source_url: "https://data.example.test/object.zip".to_string(),
        expected_sha256: "0000000000000000000000000000000000000000000000000000000000000000"
            .to_string(),
        expected_bytes,
        archive_date: "2026-03-01".to_string(),
        schema_columns: vec!["id".to_string()],
        output_dir: dir.path().join("manifest"),
    };

    let mut resolver = |_region: &str, _path: &str| {
        Err::<String, String>("no SSM resolution expected".to_string())
    };
    let error = stage_backfill_object_with_resolver(&spec, &mut resolver).unwrap_err();

    assert!(matches!(
        error,
        BackfillObjectStagingError::BytesMismatch {
            expected,
            actual
        } if expected == expected_bytes && actual == actual_bytes
    ));
    assert!(!output_object.exists());
}

#[test]
fn staging_refuses_existing_object() {
    let dir = tempfile::tempdir().expect("tempdir");
    let local_object = dir.path().join("source.zip");
    let object_bytes = b"payload";
    fs::write(&local_object, object_bytes).expect("write local object");
    let sha256 = format!("{:x}", Sha256::digest(object_bytes));
    let artifact_root = dir.path().join("artifact-root");
    let output_object = artifact_root.join("raw").join("object.zip");
    fs::create_dir_all(output_object.parent().expect("parent")).expect("mkdir");
    fs::write(&output_object, b"existing").expect("write existing staged object");
    let spec = BackfillObjectStagingSpec {
        staging_id: "existing-object".to_string(),
        artifact_root: format!("file://{}", artifact_root.display()),
        artifact_store: empty_artifact_store(),
        local_object_path: local_object,
        output_object_uri: format!("file://{}", output_object.display()),
        source_url: "https://data.example.test/object.zip".to_string(),
        expected_sha256: sha256,
        expected_bytes: object_bytes.len() as u64,
        archive_date: "2026-03-01".to_string(),
        schema_columns: vec!["id".to_string()],
        output_dir: dir.path().join("manifest"),
    };

    let mut resolver = |_region: &str, _path: &str| {
        Err::<String, String>("no SSM resolution expected".to_string())
    };
    let error = stage_backfill_object_with_resolver(&spec, &mut resolver).unwrap_err();

    assert!(matches!(
        error,
        BackfillObjectStagingError::OutputObjectAlreadyExists { .. }
    ));
}
