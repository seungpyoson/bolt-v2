//! Operator entrypoint glue, lifted out of `main` so it is unit-testable.
//!
//! Everything that identifies the dataset (accepted object, source proof, run
//! manifest, instrument spec) comes from a config-driven [`RunSpec`]; the only
//! runtime inputs are the raw `.csv.gz` bytes of the accepted object and an
//! output directory. [`run_from_run_spec`] re-verifies the object SHA-256
//! against the run-spec before any normalization, decompresses the object,
//! accepts the source proof and binds the object through the ledger, guarantees
//! a clean catalog root, runs the backtest, and writes the accepted proof plus
//! result contract as JSON artifacts.

use std::{
    fs,
    io::Read,
    path::{Component, Path, PathBuf},
};

use anyhow::{Context, Result, ensure};
use bytes::Bytes;
use nautilus_persistence::parquet::create_object_store_from_path;
use object_store::{ObjectStoreExt, path::Path as ObjectPath};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::{
    canonical_trades::{CanonicalInstrumentIdentity, TRANSFORM_IDENTITY},
    catalog_projection::SpotInstrumentSpec,
    conversion_boundary::{
        ConversionCheckpoint, ConversionFingerprint, ConversionOutputState,
        inspect_conversion_output, write_completed_conversion_artifacts,
        write_conversion_checkpoint,
    },
    result_contract::ResultArtifactUris,
    run_manifest::{BacktestingRunManifest, CATALOG_FS_PROTOCOL_NONE},
    runner::{BacktestRunInputs, BacktestRunOutput, run_backtest},
    source_proof::{
        AcceptanceMode, IngestManifestObjectRecord, SourceProofReport, select_accepted_dataset,
    },
};

/// Canonical normalized-trades Parquet artifact filename.
pub const CANONICAL_ARTIFACT_FILE: &str = "canonical-trades.parquet";
/// NautilusTrader catalog projection sub-directory.
pub const CATALOG_DIR: &str = "nt-catalog";
/// Objective result-contract artifact filename.
pub const RESULT_CONTRACT_FILE: &str = "backtest-result-contract.json";
/// Accepted source-proof artifact filename.
pub const ACCEPTED_SOURCE_PROOF_FILE: &str = "accepted-source-proof.json";

/// Config-driven dataset facts for one operator run.
#[derive(Debug, Clone, Deserialize)]
pub struct RunSpec {
    /// Ingest capture timestamp (RFC 3339).
    pub capture_time_utc: String,
    /// Result-contract `created_at` timestamp (RFC 3339).
    pub created_at_utc: String,
    /// Operator/actor recorded as accepting the source proof.
    pub accepted_by: String,
    /// Acceptance timestamp (RFC 3339).
    pub accepted_at_utc: String,
    pub accepted_object: IngestManifestObjectRecord,
    pub source_proof: SourceProofReport,
    pub instrument_spec: SpotInstrumentSpec,
    pub identity: CanonicalInstrumentIdentity,
    pub converter: ConverterConfig,
    pub manifest: BacktestingRunManifest,
}

/// Converter identity configured with the run, then checked against the
/// compiled converter before any converted output can be reused.
#[derive(Debug, Clone, Deserialize)]
pub struct ConverterConfig {
    pub identity: String,
    pub version: String,
}

/// Artifacts produced by an operator run.
pub struct RunArtifacts {
    /// SHA-256 re-computed from the supplied object bytes.
    pub verified_sha256: String,
    /// The accepted (status-stamped) source proof that was written.
    pub accepted_source_proof: SourceProofReport,
    pub canonical_artifact_path: PathBuf,
    pub catalog_root: PathBuf,
    pub proof_path: PathBuf,
    pub contract_path: PathBuf,
    pub conversion_manifest_path: PathBuf,
    pub conversion_checkpoint_path: PathBuf,
    pub catalog_metadata_path: PathBuf,
    pub output: BacktestRunOutput,
}

/// One local artifact copied to the configured publish prefix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishedArtifact {
    pub local_path: PathBuf,
    pub published_uri: String,
    pub bytes: u64,
    pub sha256: String,
}

/// A verified local run plus the artifacts published to its configured prefix.
pub struct PublishedRunArtifacts {
    pub run: RunArtifacts,
    pub published_artifacts: Vec<PublishedArtifact>,
}

/// Parse an RFC 3339 timestamp into Unix nanoseconds.
///
/// # Errors
///
/// Returns an error if `value` is not RFC 3339 or is out of nanosecond range.
pub fn rfc3339_to_nanos(value: &str) -> Result<i64> {
    chrono::DateTime::parse_from_rfc3339(value)
        .with_context(|| format!("invalid RFC3339 timestamp {value:?}"))?
        .timestamp_nanos_opt()
        .context("timestamp out of representable nanosecond range")
}

fn portable_artifact_uri(prefix: &str, artifact: &str) -> String {
    format!("{}/{}", prefix.trim_end_matches('/'), artifact)
}

fn portable_artifact_uris(manifest: &BacktestingRunManifest) -> ResultArtifactUris {
    ResultArtifactUris {
        source_proof_uri: portable_artifact_uri(
            &manifest.output_prefix,
            ACCEPTED_SOURCE_PROOF_FILE,
        ),
        canonical_table_uri: portable_artifact_uri(
            &manifest.output_prefix,
            CANONICAL_ARTIFACT_FILE,
        ),
        nt_catalog_uri: portable_artifact_uri(&manifest.output_prefix, CATALOG_DIR),
        result_contract_uri: portable_artifact_uri(&manifest.output_prefix, RESULT_CONTRACT_FILE),
    }
}

fn redact_operator_contract(output: &mut BacktestRunOutput) {
    output.contract.nt_result.machine_id = "operator-attested-redacted".to_string();
}

fn validate_converter_config(converter: &ConverterConfig) -> Result<()> {
    ensure!(
        converter.identity == TRANSFORM_IDENTITY,
        "run-spec converter.identity {:?} does not match compiled converter {TRANSFORM_IDENTITY:?}",
        converter.identity
    );
    ensure!(
        !converter.version.trim().is_empty(),
        "run-spec converter.version must not be empty"
    );
    Ok(())
}

/// Run the vertical slice from a parsed [`RunSpec`] and the raw `.csv.gz` bytes
/// of the accepted object, writing artifacts under `output_dir`.
///
/// # Errors
///
/// Returns an error if the object hash does not match the run-spec, the gzip
/// cannot be decompressed, source-proof acceptance / ledger selection fails, or
/// any backtest gate fails.
pub fn run_from_run_spec(
    spec: &RunSpec,
    gz_bytes: &[u8],
    output_dir: &Path,
) -> Result<RunArtifacts> {
    validate_converter_config(&spec.converter)?;

    // Re-verify the accepted object content hash against the run-spec, so raw
    // staged data can never reach the backtest without matching the pinned hash.
    let mut hasher = Sha256::new();
    hasher.update(gz_bytes);
    let verified_sha256 = hex::encode(hasher.finalize());
    ensure!(
        verified_sha256 == spec.accepted_object.sha256,
        "object SHA-256 {verified_sha256} does not match run-spec {}",
        spec.accepted_object.sha256
    );

    // Decompress to CSV text.
    let mut csv_text = String::new();
    flate2::read::GzDecoder::new(gz_bytes)
        .read_to_string(&mut csv_text)
        .context("decompress gzip object")?;

    // Gate 1: accept the source proof and bind the object via the ledger.
    let accepted_proof = spec
        .source_proof
        .clone()
        .accept(
            AcceptanceMode::Manual,
            spec.accepted_by.clone(),
            spec.accepted_at_utc.clone(),
        )
        .map_err(|error| anyhow::anyhow!("source-proof acceptance failed: {error}"))?;
    let accepted =
        select_accepted_dataset(&accepted_proof, &spec.accepted_object, &verified_sha256)
            .map_err(|error| anyhow::anyhow!("accepted-data ledger rejected object: {error}"))?;

    let conversion_fingerprint = ConversionFingerprint {
        source_proof_id: accepted.source_proof_id.clone(),
        source_proof_version: accepted.source_proof_version,
        accepted_object_sha256: accepted.accepted_object_sha256.clone(),
        converter_identity: spec.converter.identity.clone(),
        converter_version: spec.converter.version.clone(),
    };
    match inspect_conversion_output(output_dir, &conversion_fingerprint)? {
        ConversionOutputState::CleanNew
        | ConversionOutputState::ResumeFromCheckpoint { .. }
        | ConversionOutputState::Complete { .. } => {}
    }

    fs::create_dir_all(output_dir)
        .with_context(|| format!("create output dir {}", output_dir.display()))?;
    let canonical_path = output_dir.join(CANONICAL_ARTIFACT_FILE);
    let catalog_root = output_dir.join(CATALOG_DIR);
    for stale_completed_artifact in [
        crate::conversion_boundary::CONVERSION_MANIFEST_FILE,
        crate::conversion_boundary::CATALOG_METADATA_FILE,
    ] {
        let path = output_dir.join(stale_completed_artifact);
        if path.exists() {
            fs::remove_file(&path).with_context(|| format!("remove {}", path.display()))?;
        }
    }
    write_conversion_checkpoint(
        output_dir,
        &ConversionCheckpoint::started(conversion_fingerprint, spec.created_at_utc.clone()),
    )?;
    // Start every local run from a clean catalog after the output prefix has
    // been proven clean, idempotent, or resumable. NautilusTrader's
    // `write_to_parquet` skips writing when a file for the same
    // instrument/interval already exists, so an ungoverned leftover projection
    // must never be silently stamped with a new source proof.
    if catalog_root.exists() {
        fs::remove_dir_all(&catalog_root)
            .with_context(|| format!("clean catalog root {}", catalog_root.display()))?;
    }
    let catalog_path = catalog_root
        .to_str()
        .context("catalog path is not valid UTF-8")?
        .to_string();
    let contract_path = output_dir.join(RESULT_CONTRACT_FILE);
    let proof_path = output_dir.join(ACCEPTED_SOURCE_PROOF_FILE);

    // Bind the manifest catalog input to the local projection root.
    let contract_manifest_hash = spec.manifest.manifest_hash();
    let mut manifest = spec.manifest.clone();
    manifest.catalog_input.catalog_path = catalog_path.clone();
    manifest.catalog_input.catalog_fs_protocol = CATALOG_FS_PROTOCOL_NONE.to_string();
    manifest.catalog_input.catalog_fs_storage_options.clear();
    manifest
        .catalog_input
        .catalog_fs_rust_storage_options
        .clear();
    let artifact_uris = portable_artifact_uris(&manifest);

    let mut output = run_backtest(BacktestRunInputs {
        accepted: &accepted,
        identity: &spec.identity,
        instrument_spec: &spec.instrument_spec,
        csv_text: &csv_text,
        capture_time_nanos: rfc3339_to_nanos(&spec.capture_time_utc)?,
        manifest: &manifest,
        contract_manifest_hash: &contract_manifest_hash,
        converter_identity: &spec.converter.identity,
        converter_version: &spec.converter.version,
        canonical_artifact_path: &canonical_path,
        catalog_root: &catalog_root,
        created_at: &spec.created_at_utc,
        artifact_uris,
    })?;
    redact_operator_contract(&mut output);

    fs::write(
        &proof_path,
        serde_json::to_string_pretty(&accepted_proof).context("serialize accepted source proof")?,
    )
    .with_context(|| format!("write {}", proof_path.display()))?;
    fs::write(
        &contract_path,
        serde_json::to_string_pretty(&output.contract).context("serialize result contract")?,
    )
    .with_context(|| format!("write {}", contract_path.display()))?;
    write_completed_conversion_artifacts(
        output_dir,
        &output.conversion_manifest,
        &output.conversion_checkpoint,
        &output.conversion_catalog_metadata,
    )?;

    Ok(RunArtifacts {
        verified_sha256,
        accepted_source_proof: accepted_proof,
        canonical_artifact_path: canonical_path,
        catalog_root,
        proof_path,
        contract_path,
        conversion_manifest_path: output_dir
            .join(crate::conversion_boundary::CONVERSION_MANIFEST_FILE),
        conversion_checkpoint_path: output_dir
            .join(crate::conversion_boundary::CONVERSION_CHECKPOINT_FILE),
        catalog_metadata_path: output_dir.join(crate::conversion_boundary::CATALOG_METADATA_FILE),
        output,
    })
}

/// Run the vertical slice locally, then publish the verified artifact tree to
/// `manifest.output_prefix`.
///
/// # Errors
///
/// Returns an error if the local run fails or if any artifact cannot be copied
/// to the configured output prefix.
pub fn run_from_run_spec_and_publish(
    spec: &RunSpec,
    gz_bytes: &[u8],
    output_dir: &Path,
) -> Result<PublishedRunArtifacts> {
    let run = run_from_run_spec(spec, gz_bytes, output_dir)?;
    let published_artifacts = publish_output_artifacts(output_dir, &spec.manifest.output_prefix)?;
    Ok(PublishedRunArtifacts {
        run,
        published_artifacts,
    })
}

/// Publish every file under `output_dir` to `output_prefix`, preserving the
/// relative artifact tree.
///
/// # Errors
///
/// Returns an error if `output_dir` is not a directory, if the prefix cannot be
/// opened by NT's object-store support, or if any artifact write fails.
pub fn publish_output_artifacts(
    output_dir: &Path,
    output_prefix: &str,
) -> Result<Vec<PublishedArtifact>> {
    ensure!(
        output_dir.is_dir(),
        "output directory does not exist: {}",
        output_dir.display()
    );
    let files = collect_output_files(output_dir)?;
    ensure!(
        !files.is_empty(),
        "output directory has no artifacts: {}",
        output_dir.display()
    );

    ensure_local_publish_root_exists(output_prefix)?;
    let (object_store, base_path, _) = create_object_store_from_path(output_prefix, None)
        .with_context(|| format!("open output prefix {output_prefix:?}"))?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("build object-store runtime")?;
    let normalized_prefix = output_prefix.trim_end_matches('/');

    let mut published = Vec::with_capacity(files.len());
    for local_path in files {
        let relative = artifact_relative_path(output_dir, &local_path)?;
        let bytes =
            fs::read(&local_path).with_context(|| format!("read {}", local_path.display()))?;
        let byte_len = bytes.len() as u64;
        let mut hasher = Sha256::new();
        hasher.update(&bytes);
        let sha256 = hex::encode(hasher.finalize());
        let object_key = if base_path.is_empty() {
            relative.clone()
        } else {
            format!("{}/{}", base_path.trim_end_matches('/'), relative)
        };
        let object_path = ObjectPath::from(object_key);
        runtime
            .block_on(object_store.put(&object_path, Bytes::from(bytes).into()))
            .with_context(|| format!("publish artifact {relative} to {output_prefix}"))?;
        published.push(PublishedArtifact {
            local_path,
            published_uri: format!("{normalized_prefix}/{relative}"),
            bytes: byte_len,
            sha256,
        });
    }

    Ok(published)
}

fn ensure_local_publish_root_exists(output_prefix: &str) -> Result<()> {
    let local_root = if let Some(path) = output_prefix.strip_prefix("file://") {
        Some(Path::new(path))
    } else if output_prefix.contains("://") {
        None
    } else {
        Some(Path::new(output_prefix))
    };
    if let Some(root) = local_root {
        fs::create_dir_all(root)
            .with_context(|| format!("create local publish root {}", root.display()))?;
    }
    Ok(())
}

fn collect_output_files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    collect_output_files_inner(root, &mut files)?;
    files.sort();
    Ok(files)
}

fn collect_output_files_inner(path: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(path).with_context(|| format!("read dir {}", path.display()))? {
        let entry = entry.with_context(|| format!("read dir entry under {}", path.display()))?;
        let entry_path = entry.path();
        let file_type = entry
            .file_type()
            .with_context(|| format!("read file type {}", entry_path.display()))?;
        if file_type.is_dir() {
            collect_output_files_inner(&entry_path, files)?;
        } else if file_type.is_file() {
            files.push(entry_path);
        }
    }
    Ok(())
}

fn artifact_relative_path(root: &Path, file: &Path) -> Result<String> {
    let relative = file
        .strip_prefix(root)
        .with_context(|| format!("artifact {} is outside {}", file.display(), root.display()))?;
    let mut components = Vec::new();
    for component in relative.components() {
        match component {
            Component::Normal(value) => components.push(value.to_string_lossy().to_string()),
            other => anyhow::bail!("unsupported artifact path component {other:?}"),
        }
    }
    ensure!(
        !components.is_empty(),
        "artifact path has no relative components: {}",
        file.display()
    );
    Ok(components.join("/"))
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use flate2::{Compression, write::GzEncoder};

    use super::*;
    use crate::conversion_boundary::{
        CATALOG_METADATA_FILE, CONVERSION_CHECKPOINT_FILE, CONVERSION_MANIFEST_FILE,
        ConversionCatalogMetadata, ConversionCheckpoint, ConversionManifest,
    };
    use crate::result_contract::BacktestResultContract;

    const COMMITTED_RUN_SPEC: &str = include_str!(
        "../../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-run-spec.bnbusdc-2026-03-01.toml"
    );
    const COMMITTED_RESULT_CONTRACT: &str = include_str!(
        "../../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-result-contract.bnbusdc-2026-03-01.json"
    );
    const COMMITTED_ACCEPTED_PROOF: &str = include_str!(
        "../../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-accepted-source-proof.bnbusdc-2026-03-01.json"
    );
    const COMMITTED_SOURCE_BINDINGS: &str = include_str!(
        "../../../specs/023-nt-research-analytics-platform/reference/backfill-source-bindings.v1.toml"
    );

    const SAMPLE_CSV: &str = "id,timestamp,price,volume,side,rpi\n\
        1,1772323201665,617.2,0.3,buy,0\n\
        2,1772323312219,617.9,0.1456,sell,0\n\
        3,1772323312236,617,0.1544,sell,0\n";

    fn gzip(text: &str) -> Vec<u8> {
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(text.as_bytes()).expect("gzip write");
        encoder.finish().expect("gzip finish")
    }

    fn sha256_hex(bytes: &[u8]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        hex::encode(hasher.finalize())
    }

    /// The committed run-spec, with the accepted-object hash rebound to a locally
    /// reproducible synthetic object (the real staged object is not committed).
    fn run_spec_for(gz_bytes: &[u8]) -> RunSpec {
        let mut spec: RunSpec =
            toml::from_str(COMMITTED_RUN_SPEC).expect("committed run-spec parses");
        let object_hash = sha256_hex(gz_bytes);
        spec.accepted_object.sha256 = object_hash.clone();
        spec.accepted_object.bytes = gz_bytes.len() as u64;
        spec.source_proof.raw_sample_hash = object_hash;
        spec
    }

    #[test]
    fn run_from_run_spec_produces_artifacts() {
        let gz = gzip(SAMPLE_CSV);
        let spec = run_spec_for(&gz);
        let dir = tempfile::TempDir::new().unwrap();
        let artifacts = run_from_run_spec(&spec, &gz, dir.path()).expect("operator run");
        assert_eq!(artifacts.output.read_back_count, 3);
        assert!(artifacts.contract_path.exists(), "result contract written");
        assert!(artifacts.proof_path.exists(), "accepted proof written");
        artifacts
            .output
            .contract
            .validate()
            .expect("contract valid");
        // The written contract round-trips back to the same type.
        let contract_json = fs::read_to_string(&artifacts.contract_path).unwrap();
        let parsed: BacktestResultContract = serde_json::from_str(&contract_json).unwrap();
        assert_eq!(parsed, artifacts.output.contract);
    }

    #[test]
    fn run_from_run_spec_writes_portable_redacted_contract() {
        let gz = gzip(SAMPLE_CSV);
        let spec = run_spec_for(&gz);
        let dir = tempfile::TempDir::new().unwrap();
        let artifacts = run_from_run_spec(&spec, &gz, dir.path()).expect("operator run");

        let contract_json = fs::read_to_string(&artifacts.contract_path).unwrap();
        let parsed: BacktestResultContract = serde_json::from_str(&contract_json).unwrap();
        assert_eq!(parsed.nt_result.machine_id, "operator-attested-redacted");
        for uri in [
            &parsed.artifact_uris.source_proof_uri,
            &parsed.artifact_uris.canonical_table_uri,
            &parsed.artifact_uris.nt_catalog_uri,
            &parsed.artifact_uris.result_contract_uri,
        ] {
            assert!(
                uri.starts_with("s3://bolt-parquet/nt-research-analytics/backtests/"),
                "{uri}"
            );
            assert!(
                !uri.contains(dir.path().to_string_lossy().as_ref()),
                "{uri}"
            );
        }
    }

    #[test]
    fn run_from_run_spec_contract_manifest_hash_is_portable_run_spec_hash() {
        let gz = gzip(SAMPLE_CSV);
        let spec = run_spec_for(&gz);
        let expected_manifest_hash = spec.manifest.manifest_hash();
        let first_dir = tempfile::TempDir::new().unwrap();
        let second_dir = tempfile::TempDir::new().unwrap();

        let first = run_from_run_spec(&spec, &gz, first_dir.path()).expect("first operator run");
        let second = run_from_run_spec(&spec, &gz, second_dir.path()).expect("second operator run");

        assert_eq!(
            first.output.contract.manifest_hash, expected_manifest_hash,
            "contract must bind the portable submitted run-spec manifest"
        );
        assert_eq!(
            second.output.contract.manifest_hash, expected_manifest_hash,
            "equivalent runs in different local output dirs must have the same manifest hash"
        );
    }

    #[test]
    fn run_from_run_spec_writes_conversion_artifacts_and_contract_binds_them() {
        let gz = gzip(SAMPLE_CSV);
        let spec = run_spec_for(&gz);
        let dir = tempfile::TempDir::new().unwrap();
        let artifacts = run_from_run_spec(&spec, &gz, dir.path()).expect("operator run");

        let manifest_path = dir.path().join(CONVERSION_MANIFEST_FILE);
        let checkpoint_path = dir.path().join(CONVERSION_CHECKPOINT_FILE);
        let metadata_path = dir.path().join(CATALOG_METADATA_FILE);
        assert!(manifest_path.exists(), "conversion manifest written");
        assert!(checkpoint_path.exists(), "conversion checkpoint written");
        assert!(metadata_path.exists(), "catalog metadata written");

        let manifest: ConversionManifest =
            serde_json::from_str(&fs::read_to_string(&manifest_path).unwrap()).unwrap();
        let checkpoint: ConversionCheckpoint =
            serde_json::from_str(&fs::read_to_string(&checkpoint_path).unwrap()).unwrap();
        let metadata: ConversionCatalogMetadata =
            serde_json::from_str(&fs::read_to_string(&metadata_path).unwrap()).unwrap();
        let manifest_hash = manifest.content_hash().unwrap();
        let checkpoint_hash = checkpoint.content_hash().unwrap();

        assert_eq!(metadata.manifest_hash, manifest_hash);
        assert_eq!(metadata.checkpoint_hash, checkpoint_hash);
        assert_eq!(
            metadata.catalog_hash,
            artifacts.output.projection.catalog_hash
        );
        assert_eq!(
            metadata.output_catalog_uri,
            artifacts.output.contract.artifact_uris.nt_catalog_uri
        );
        assert_eq!(
            metadata.execution_catalog_uri,
            dir.path().join(CATALOG_DIR).to_str().unwrap()
        );
        assert!(
            !metadata.direct_s3_catalog_access_proven,
            "local operator path must not claim direct S3 BacktestNode consumption"
        );
        assert_eq!(
            artifacts.output.contract.converter_identity,
            manifest.fingerprint.converter_identity
        );
        assert_eq!(
            artifacts.output.contract.converter_version,
            manifest.fingerprint.converter_version
        );
        assert_eq!(
            artifacts.output.contract.conversion_manifest_hash,
            manifest_hash
        );
        assert_eq!(
            artifacts.output.contract.conversion_checkpoint_hash,
            checkpoint_hash
        );
    }

    #[test]
    fn run_from_run_spec_rejects_tampered_object() {
        // The committed run-spec pins the real (uncommitted) object hash; feeding
        // it the synthetic bytes must trip the SHA-256 re-verification.
        let gz = gzip(SAMPLE_CSV);
        let spec: RunSpec = toml::from_str(COMMITTED_RUN_SPEC).expect("parse");
        let dir = tempfile::TempDir::new().unwrap();
        let err = run_from_run_spec(&spec, &gz, dir.path())
            .err()
            .expect("tampered object must be rejected");
        assert!(err.to_string().contains("SHA-256"), "{err}");
    }

    #[test]
    fn run_from_run_spec_rejects_dirty_output_without_conversion_checkpoint() {
        let gz = gzip(SAMPLE_CSV);
        let spec = run_spec_for(&gz);
        let dir = tempfile::TempDir::new().unwrap();
        fs::write(dir.path().join("stale.parquet"), b"stale").unwrap();

        let err = run_from_run_spec(&spec, &gz, dir.path())
            .err()
            .expect("dirty output must be rejected");

        assert!(err.to_string().contains("dirty conversion output"), "{err}");
        assert!(
            dir.path().join("stale.parquet").exists(),
            "dirty evidence must be preserved for inspection"
        );
    }

    #[test]
    fn run_from_run_spec_cleans_stale_catalog() {
        let gz = gzip(SAMPLE_CSV);
        let spec = run_spec_for(&gz);
        let dir = tempfile::TempDir::new().unwrap();
        run_from_run_spec(&spec, &gz, dir.path()).expect("first run");
        // A second run into the same output dir must clean the stale catalog and
        // succeed, not trip the dirty-catalog guard.
        run_from_run_spec(&spec, &gz, dir.path()).expect("second run cleans stale catalog");
    }

    #[test]
    fn run_from_run_spec_and_publish_copies_artifacts_to_configured_prefix() {
        let gz = gzip(SAMPLE_CSV);
        let mut spec = run_spec_for(&gz);
        let local_dir = tempfile::TempDir::new().unwrap();
        let published_root = tempfile::TempDir::new().unwrap();
        let artifact_root = format!("file://{}", published_root.path().display());
        spec.manifest.artifact_root = artifact_root.clone();
        spec.manifest.output_prefix = format!("{artifact_root}/backtests/published-run");

        let published =
            run_from_run_spec_and_publish(&spec, &gz, local_dir.path()).expect("published run");

        assert_eq!(published.run.output.read_back_count, 3);
        assert!(
            !published.published_artifacts.is_empty(),
            "artifact publish set must be reported"
        );
        for artifact in &published.published_artifacts {
            assert!(
                artifact
                    .published_uri
                    .starts_with(&spec.manifest.output_prefix),
                "{}",
                artifact.published_uri
            );
        }
        assert_eq!(
            fs::read_to_string(
                published_root
                    .path()
                    .join("backtests/published-run/conversion-manifest.json"),
            )
            .expect("published manifest"),
            fs::read_to_string(local_dir.path().join(CONVERSION_MANIFEST_FILE))
                .expect("local manifest")
        );
        assert!(
            published_root
                .path()
                .join("backtests/published-run/nt-catalog/data/trades/BNBUSDC.BYBIT")
                .is_dir(),
            "published NT catalog tree must include trade data"
        );
    }

    #[test]
    fn committed_run_spec_deserializes() {
        let spec: RunSpec = toml::from_str(COMMITTED_RUN_SPEC).expect("run-spec parses");
        assert_eq!(
            spec.source_proof.source_proof_id,
            "source-proof-bybit-spot-tick-trades"
        );
        assert_eq!(
            spec.manifest.catalog_input.nt_instrument_id,
            "BNBUSDC.BYBIT"
        );
    }

    #[test]
    fn committed_run_spec_binds_converter_identity() {
        let spec: toml::Value = toml::from_str(COMMITTED_RUN_SPEC).expect("run-spec parses");
        assert_eq!(
            spec["converter"]["identity"].as_str(),
            Some(crate::canonical_trades::TRANSFORM_IDENTITY)
        );
        assert_eq!(spec["converter"]["version"].as_str(), Some("1"));
    }

    #[test]
    fn committed_run_spec_source_binding_exists_in_registry() {
        let spec: RunSpec = toml::from_str(COMMITTED_RUN_SPEC).expect("run-spec parses");
        let registry = toml::from_str::<toml::Table>(COMMITTED_SOURCE_BINDINGS)
            .expect("source binding registry parses");
        let source_bindings = registry
            .get("source_binding")
            .and_then(toml::Value::as_array)
            .expect("source_binding array");
        assert!(
            source_bindings.iter().any(|binding| {
                binding.get("key").and_then(toml::Value::as_str)
                    == Some(spec.source_proof.source_binding.as_str())
            }),
            "missing source binding key {}",
            spec.source_proof.source_binding
        );
    }

    #[test]
    fn committed_result_contract_deserializes() {
        let contract: BacktestResultContract =
            serde_json::from_str(COMMITTED_RESULT_CONTRACT).expect("result contract parses");
        contract
            .validate()
            .expect("committed contract is objective");
    }

    #[test]
    fn committed_result_contract_uses_portable_reference_uris() {
        let contract: BacktestResultContract =
            serde_json::from_str(COMMITTED_RESULT_CONTRACT).expect("result contract parses");
        assert_ne!(contract.nt_result.machine_id, "SP-MB-Pro.local");
        for uri in [
            &contract.artifact_uris.source_proof_uri,
            &contract.artifact_uris.canonical_table_uri,
            &contract.artifact_uris.nt_catalog_uri,
            &contract.artifact_uris.result_contract_uri,
        ] {
            assert!(!uri.starts_with("/private/tmp/"), "{uri}");
        }
        assert!(contract.claim_limits.iter().any(|limit| {
            limit.contains("operator-attested") && limit.contains("not reproduced in CI")
        }));
    }

    #[test]
    fn committed_accepted_proof_deserializes() {
        let proof: SourceProofReport =
            serde_json::from_str(COMMITTED_ACCEPTED_PROOF).expect("accepted proof parses");
        assert!(proof.is_accepted(), "committed proof is accepted");
    }
}
