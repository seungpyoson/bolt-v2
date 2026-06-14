use std::{collections::BTreeMap, fs};

use backtesting_vertical_slice::{
    run_manifest::ManifestArtifactStore,
    source_proof_evidence_staging::{
        SourceProofEvidenceStagingError, SourceProofEvidenceStagingFile,
        SourceProofEvidenceStagingSpec, stage_source_proof_evidence_with_resolver,
    },
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
fn stages_multiple_source_proof_evidence_files_create_only() {
    let dir = tempfile::tempdir().expect("tempdir");
    let artifact_root = dir.path().join("artifact-root");
    let schema_path = dir.path().join("schema.json");
    let license_path = dir.path().join("license-attestation.txt");
    let schema_bytes = br#"{"columns":["trade_id","price"]}"#;
    let license_bytes = b"operator attests written approval exists";
    fs::write(&schema_path, schema_bytes).expect("write schema");
    fs::write(&license_path, license_bytes).expect("write license evidence");
    let schema_sha256 = format!("{:x}", Sha256::digest(schema_bytes));
    let license_sha256 = format!("{:x}", Sha256::digest(license_bytes));
    let output_dir = dir.path().join("manifest");
    let schema_uri = format!(
        "file://{}",
        artifact_root
            .join("source-proofs/v1/source_binding=synthetic/proof=proof/version=1/schema.json")
            .display()
    );
    let license_uri = format!(
        "file://{}",
        artifact_root
            .join("source-proofs/v1/source_binding=synthetic/proof=proof/version=1/license.txt",)
            .display()
    );

    let spec = SourceProofEvidenceStagingSpec {
        staging_id: "source-proof-evidence-pack".to_string(),
        artifact_root: format!("file://{}", artifact_root.display()),
        artifact_store: empty_artifact_store(),
        evidence_files: vec![
            SourceProofEvidenceStagingFile {
                evidence_kind: "schema_sample".to_string(),
                local_path: schema_path,
                output_uri: schema_uri.clone(),
                expected_sha256: schema_sha256.clone(),
                expected_bytes: schema_bytes.len() as u64,
            },
            SourceProofEvidenceStagingFile {
                evidence_kind: "license".to_string(),
                local_path: license_path,
                output_uri: license_uri.clone(),
                expected_sha256: license_sha256.clone(),
                expected_bytes: license_bytes.len() as u64,
            },
        ],
        output_dir,
    };

    let mut resolver = |_region: &str, _path: &str| {
        Err::<String, String>("no SSM resolution expected".to_string())
    };
    let artifact =
        stage_source_proof_evidence_with_resolver(&spec, &mut resolver).expect("stage evidence");

    assert_eq!(artifact.record_count, 2);
    assert_eq!(
        fs::read(
            artifact_root.join(
                "source-proofs/v1/source_binding=synthetic/proof=proof/version=1/schema.json"
            )
        )
        .expect("read staged schema"),
        schema_bytes
    );
    assert_eq!(
        fs::read(
            artifact_root.join(
                "source-proofs/v1/source_binding=synthetic/proof=proof/version=1/license.txt",
            )
        )
        .expect("read staged license"),
        license_bytes
    );

    let manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(&artifact.manifest_path).expect("read manifest"))
            .expect("parse manifest");
    assert_eq!(manifest["manifest_id"], "source-proof-evidence-pack");
    assert_eq!(
        manifest["evidence_records"]
            .as_array()
            .expect("records")
            .len(),
        2
    );
    assert_eq!(
        manifest["evidence_records"][0]["evidence_kind"],
        "schema_sample"
    );
    assert_eq!(manifest["evidence_records"][0]["uri"], schema_uri);
    assert_eq!(manifest["evidence_records"][0]["sha256"], schema_sha256);
    assert_eq!(manifest["evidence_records"][1]["evidence_kind"], "license");
    assert_eq!(manifest["evidence_records"][1]["uri"], license_uri);
    assert_eq!(manifest["evidence_records"][1]["sha256"], license_sha256);
}

#[test]
fn rejects_hash_mismatch_before_writing_evidence() {
    let dir = tempfile::tempdir().expect("tempdir");
    let artifact_root = dir.path().join("artifact-root");
    let local_path = dir.path().join("schema.json");
    fs::write(&local_path, b"payload").expect("write local evidence");
    let output_path =
        artifact_root.join("source-proofs/v1/source_binding=x/proof=y/version=1/schema.json");
    let spec = SourceProofEvidenceStagingSpec {
        staging_id: "hash-mismatch".to_string(),
        artifact_root: format!("file://{}", artifact_root.display()),
        artifact_store: empty_artifact_store(),
        evidence_files: vec![SourceProofEvidenceStagingFile {
            evidence_kind: "schema_sample".to_string(),
            local_path,
            output_uri: format!("file://{}", output_path.display()),
            expected_sha256: "0000000000000000000000000000000000000000000000000000000000000000"
                .to_string(),
            expected_bytes: 7,
        }],
        output_dir: dir.path().join("manifest"),
    };

    let mut resolver = |_region: &str, _path: &str| {
        Err::<String, String>("no SSM resolution expected".to_string())
    };
    let error = stage_source_proof_evidence_with_resolver(&spec, &mut resolver).unwrap_err();

    assert!(matches!(
        error,
        SourceProofEvidenceStagingError::Sha256Mismatch { .. }
    ));
    assert!(!output_path.exists());
}

#[test]
fn rejects_evidence_uri_outside_source_proofs_subpath() {
    let dir = tempfile::tempdir().expect("tempdir");
    let artifact_root = dir.path().join("artifact-root");
    let local_path = dir.path().join("schema.json");
    let bytes = b"payload";
    fs::write(&local_path, bytes).expect("write local evidence");
    let spec = SourceProofEvidenceStagingSpec {
        staging_id: "wrong-subpath".to_string(),
        artifact_root: format!("file://{}", artifact_root.display()),
        artifact_store: empty_artifact_store(),
        evidence_files: vec![SourceProofEvidenceStagingFile {
            evidence_kind: "schema_sample".to_string(),
            local_path,
            output_uri: format!("file://{}", artifact_root.join("raw/object.json").display()),
            expected_sha256: format!("{:x}", Sha256::digest(bytes)),
            expected_bytes: bytes.len() as u64,
        }],
        output_dir: dir.path().join("manifest"),
    };

    let mut resolver = |_region: &str, _path: &str| {
        Err::<String, String>("no SSM resolution expected".to_string())
    };
    let error = stage_source_proof_evidence_with_resolver(&spec, &mut resolver).unwrap_err();

    assert!(matches!(
        error,
        SourceProofEvidenceStagingError::ArtifactRootMismatch { .. }
    ));
}

#[test]
fn refuses_existing_evidence_object() {
    let dir = tempfile::tempdir().expect("tempdir");
    let artifact_root = dir.path().join("artifact-root");
    let local_path = dir.path().join("schema.json");
    let bytes = b"payload";
    fs::write(&local_path, bytes).expect("write local evidence");
    let output_path =
        artifact_root.join("source-proofs/v1/source_binding=x/proof=y/version=1/schema.json");
    fs::create_dir_all(output_path.parent().expect("parent")).expect("mkdir");
    fs::write(&output_path, b"existing").expect("write existing evidence");
    let spec = SourceProofEvidenceStagingSpec {
        staging_id: "existing-evidence".to_string(),
        artifact_root: format!("file://{}", artifact_root.display()),
        artifact_store: empty_artifact_store(),
        evidence_files: vec![SourceProofEvidenceStagingFile {
            evidence_kind: "schema_sample".to_string(),
            local_path,
            output_uri: format!("file://{}", output_path.display()),
            expected_sha256: format!("{:x}", Sha256::digest(bytes)),
            expected_bytes: bytes.len() as u64,
        }],
        output_dir: dir.path().join("manifest"),
    };

    let mut resolver = |_region: &str, _path: &str| {
        Err::<String, String>("no SSM resolution expected".to_string())
    };
    let error = stage_source_proof_evidence_with_resolver(&spec, &mut resolver).unwrap_err();

    assert!(matches!(
        error,
        SourceProofEvidenceStagingError::OutputObjectAlreadyExists { .. }
    ));
}
