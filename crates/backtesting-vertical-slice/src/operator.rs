//! Operator entrypoint glue, lifted out of `main` so it is unit-testable.
//!
//! Everything that identifies the dataset (accepted object, source proof, run
//! manifest, instrument spec) comes from a config-driven [`RunSpec`]; the only
//! runtime inputs are the raw bytes of the accepted object and an
//! output directory. [`run_from_run_spec`] re-verifies the object SHA-256
//! against the run-spec before any normalization, decodes the object,
//! accepts the source proof and binds the object through the ledger, guarantees
//! a clean catalog root, runs the backtest, and writes the accepted proof,
//! artifact-local run manifest, and result contract as JSON artifacts.

use std::{
    collections::BTreeMap,
    fs,
    io::{Cursor, Read},
    path::{Component, Path, PathBuf},
};

use anyhow::{Context, Result, ensure};
use bytes::Bytes;
use nautilus_persistence::parquet::create_object_store_from_path;
use object_store::{Error as ObjectStoreError, ObjectStoreExt, PutMode, path::Path as ObjectPath};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    canonical_trades::{
        CanonicalInstrumentIdentity, CanonicalTradesTable, ConverterConfig, RawPayloadConfig,
        RawPayloadContainer, require_registered_trade_converter,
    },
    catalog_projection::{
        CatalogProjection, SpotInstrumentSpec, logical_catalog_hash, read_back_trade_ticks,
    },
    conversion_boundary::{
        CATALOG_METADATA_FILE, ConversionCatalogMetadata, ConversionCheckpoint,
        ConversionFingerprint, ConversionManifest, ConversionOutputState,
        inspect_conversion_output, write_completed_conversion_artifacts,
        write_conversion_checkpoint,
    },
    result_contract::{ResultArtifactUris, ResultContractInputs, build_result_contract},
    run_manifest::{BacktestingRunManifest, CATALOG_FS_PROTOCOL_NONE},
    runner::{
        BacktestRunInputs, BacktestRunOutput, assert_time_window_overlaps_data,
        expected_iterations, iterations_mismatch, market_structure_label,
        nt_extension_surface_claim_limits, result_contract_warnings, run_backtest,
        run_nt_backtest_node, run_purpose_label,
    },
    source_proof::{
        AcceptanceMode, AcceptedDataset, IngestManifestObjectRecord, SourceProofReport,
        select_accepted_dataset,
    },
};

/// Canonical normalized-trades Parquet artifact filename.
pub const CANONICAL_ARTIFACT_FILE: &str = "canonical-trades.parquet";
/// NautilusTrader catalog projection sub-directory.
pub const CATALOG_DIR: &str = "nt-catalog";
/// Objective result-contract artifact filename.
pub const RESULT_CONTRACT_FILE: &str = "backtest-result-contract.json";
/// Artifact-local run-manifest artifact filename.
pub const BACKTEST_RUN_MANIFEST_FILE: &str = "backtest-run-manifest.json";
/// Accepted source-proof artifact filename.
pub const ACCEPTED_SOURCE_PROOF_FILE: &str = "accepted-source-proof.json";
/// Published-catalog `BacktestNode` proof artifact filename.
pub const PUBLISHED_CATALOG_PROOF_FILE: &str = "published-catalog-proof.json";

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
    pub run_manifest_path: PathBuf,
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
    pub published_catalog_proof: Option<PublishedCatalogProof>,
}

/// Optional publish-time proof configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PublishOptions {
    pub prove_published_catalog: bool,
}

/// Evidence that NautilusTrader consumed the published catalog.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublishedCatalogProof {
    pub proof_version: String,
    pub catalog_uri: String,
    pub catalog_fs_protocol: String,
    pub direct_s3_catalog_access_proven: bool,
    pub expected_iterations: usize,
    pub nt_iterations: usize,
    pub run_config_id: Option<String>,
    pub nt_version: String,
    pub created_at: String,
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
        catalog_metadata_uri: portable_artifact_uri(&manifest.output_prefix, CATALOG_METADATA_FILE),
        result_contract_uri: portable_artifact_uri(&manifest.output_prefix, RESULT_CONTRACT_FILE),
    }
}

fn redact_operator_contract(output: &mut BacktestRunOutput) {
    output.contract.nt_result.machine_id = "operator-attested-redacted".to_string();
}

fn validate_converter_config(converter: &ConverterConfig) -> Result<()> {
    ensure!(
        !converter.version.trim().is_empty(),
        "run-spec converter.version must not be empty"
    );
    require_registered_trade_converter(&converter.identity, &converter.version)?;
    validate_raw_payload_config(&converter.raw_payload)?;
    Ok(())
}

fn validate_raw_payload_config(config: &RawPayloadConfig) -> Result<()> {
    ensure!(
        config.max_object_bytes > 0,
        "converter.raw_payload.max_object_bytes must be positive"
    );
    ensure!(
        config.max_decoded_bytes > 0,
        "converter.raw_payload.max_decoded_bytes must be positive"
    );
    match config.container {
        RawPayloadContainer::CsvGzip | RawPayloadContainer::CsvText => {
            ensure!(
                config.zip_member.is_none(),
                "converter.raw_payload.zip_member is only valid for single_csv_zip"
            );
        }
        RawPayloadContainer::SingleCsvZip => {
            ensure!(
                config
                    .zip_member
                    .as_ref()
                    .is_some_and(|member| !member.trim().is_empty()),
                "converter.raw_payload.zip_member is required for single_csv_zip"
            );
        }
    }
    Ok(())
}

fn ensure_object_within_raw_payload_limit(
    config: &RawPayloadConfig,
    object_byte_len: u64,
) -> Result<()> {
    ensure!(
        object_byte_len <= config.max_object_bytes,
        "accepted object byte length {object_byte_len} exceeds converter.raw_payload.max_object_bytes {}",
        config.max_object_bytes
    );
    Ok(())
}

fn read_limited_csv_text<R: Read>(
    reader: R,
    max_decoded_bytes: u64,
    context_label: &str,
) -> Result<String> {
    let read_limit = max_decoded_bytes
        .checked_add(1)
        .context("converter.raw_payload.max_decoded_bytes is too large")?;
    let mut limited = reader.take(read_limit);
    let mut bytes = Vec::new();
    limited
        .read_to_end(&mut bytes)
        .with_context(|| format!("decode {context_label}"))?;
    ensure!(
        bytes.len() as u64 <= max_decoded_bytes,
        "decoded CSV byte length {} exceeds converter.raw_payload.max_decoded_bytes {max_decoded_bytes}",
        bytes.len()
    );
    String::from_utf8(bytes).with_context(|| format!("decode {context_label} as UTF-8 CSV"))
}

fn decode_csv_payload(config: &RawPayloadConfig, object_bytes: &[u8]) -> Result<String> {
    validate_raw_payload_config(config)?;
    match config.container {
        RawPayloadContainer::CsvGzip => read_limited_csv_text(
            flate2::read::GzDecoder::new(object_bytes),
            config.max_decoded_bytes,
            "gzip csv object",
        ),
        RawPayloadContainer::CsvText => read_limited_csv_text(
            Cursor::new(object_bytes),
            config.max_decoded_bytes,
            "plain csv object",
        ),
        RawPayloadContainer::SingleCsvZip => {
            let member_name = config
                .zip_member
                .as_deref()
                .context("converter.raw_payload.zip_member is required for single_csv_zip")?;
            let cursor = Cursor::new(object_bytes);
            let mut archive = zip::ZipArchive::new(cursor).context("open zip csv object")?;
            let member = archive
                .by_name(member_name)
                .with_context(|| format!("open zip member {member_name:?}"))?;
            ensure!(
                !member.is_dir(),
                "configured zip member {member_name:?} is a directory"
            );
            read_limited_csv_text(
                member,
                config.max_decoded_bytes,
                &format!("zip member {member_name:?}"),
            )
        }
    }
}

struct CompletedOutputInputs<'a> {
    verified_sha256: String,
    accepted_source_proof: SourceProofReport,
    accepted: &'a AcceptedDataset,
    canonical_artifact_path: PathBuf,
    catalog_root: PathBuf,
    proof_path: PathBuf,
    contract_path: PathBuf,
    run_manifest_path: PathBuf,
    conversion_manifest_path: PathBuf,
    conversion_checkpoint_path: PathBuf,
    catalog_metadata_path: PathBuf,
    manifest: BacktestingRunManifest,
    contract_manifest_hash: String,
    conversion_manifest_hash: String,
    conversion_checkpoint_hash: String,
    expected_catalog_hash: String,
    artifact_uris: ResultArtifactUris,
    created_at: &'a str,
    spec_manifest: &'a BacktestingRunManifest,
}

fn run_from_completed_output(inputs: CompletedOutputInputs<'_>) -> Result<RunArtifacts> {
    let conversion_checkpoint: ConversionCheckpoint =
        read_json_artifact(&inputs.conversion_checkpoint_path)?;
    ensure!(
        conversion_checkpoint.content_hash()? == inputs.conversion_checkpoint_hash,
        "completed conversion checkpoint hash changed after inspection"
    );
    let conversion_manifest: ConversionManifest =
        read_json_artifact(&inputs.conversion_manifest_path)?;
    ensure!(
        conversion_manifest.content_hash()? == inputs.conversion_manifest_hash,
        "completed conversion manifest hash changed after inspection"
    );
    ensure!(
        conversion_manifest.output_catalog_uri == inputs.artifact_uris.nt_catalog_uri,
        "completed conversion output_catalog_uri does not match current run manifest"
    );
    let conversion_catalog_metadata: ConversionCatalogMetadata =
        read_json_artifact(&inputs.catalog_metadata_path)?;
    ensure!(
        conversion_catalog_metadata.checkpoint_hash == inputs.conversion_checkpoint_hash,
        "completed catalog metadata checkpoint_hash mismatch"
    );
    ensure!(
        conversion_catalog_metadata.manifest_hash == inputs.conversion_manifest_hash,
        "completed catalog metadata manifest_hash mismatch"
    );
    ensure!(
        conversion_catalog_metadata.catalog_hash == inputs.expected_catalog_hash,
        "completed catalog metadata catalog_hash mismatch"
    );
    let conversion_catalog_metadata_hash = conversion_catalog_metadata
        .content_hash()
        .context("hash completed catalog metadata")?;

    let actual_catalog_hash = logical_catalog_hash(&inputs.catalog_root)
        .with_context(|| format!("verify catalog hash {}", inputs.catalog_root.display()))?;
    ensure!(
        actual_catalog_hash == inputs.expected_catalog_hash,
        "completed NT catalog hash mismatch: expected {:?}, got {:?}",
        inputs.expected_catalog_hash,
        actual_catalog_hash
    );

    let canonical_table =
        CanonicalTradesTable::read_parquet(&inputs.canonical_artifact_path, inputs.accepted)?;
    ensure!(
        canonical_table.rows.len() == conversion_manifest.canonical_rows,
        "completed canonical row count mismatch: manifest has {}, parquet has {}",
        conversion_manifest.canonical_rows,
        canonical_table.rows.len()
    );
    ensure!(
        canonical_table.schema_version == conversion_manifest.normalized_schema_version,
        "completed canonical schema mismatch"
    );
    ensure!(
        conversion_manifest.nt_instrument_id == inputs.manifest.catalog_input.nt_instrument_id,
        "completed conversion instrument does not match run manifest"
    );
    assert_time_window_overlaps_data(&inputs.manifest, &canonical_table)?;

    let read_back =
        read_back_trade_ticks(&inputs.catalog_root, &conversion_manifest.nt_instrument_id)
            .context("read back completed NT catalog")?;
    ensure!(
        read_back.len() == canonical_table.rows.len(),
        "completed NT catalog read-back count {} does not match canonical rows {}",
        read_back.len(),
        canonical_table.rows.len()
    );

    let nt_result = run_nt_backtest_node(&inputs.manifest)?;
    let expected = expected_iterations(
        &canonical_table.rows,
        inputs.manifest.start_time,
        inputs.manifest.end_time,
    );
    if let Some(reason) = iterations_mismatch(nt_result.iterations, expected) {
        anyhow::bail!("backtest did not consume the accepted data: {reason}");
    }

    let mut claim_limits = canonical_table.forbidden_claims.clone();
    claim_limits.extend(nt_extension_surface_claim_limits(&inputs.manifest)?);
    let contract = build_result_contract(ResultContractInputs {
        run_id: &inputs.manifest.run_id,
        source_proof_id: &inputs.accepted.source_proof_id,
        source_proof_version: inputs.accepted.source_proof_version,
        manifest_hash: &inputs.contract_manifest_hash,
        acceptance_mode: inputs.accepted.acceptance_mode,
        accepted_by: &inputs.accepted.accepted_by,
        accepted_at: &inputs.accepted.accepted_at,
        accepted_object_sha256: &inputs.accepted.accepted_object_sha256,
        converter_identity: &conversion_manifest.fingerprint.converter_identity,
        converter_version: &conversion_manifest.fingerprint.converter_version,
        converter_config_hash: &conversion_manifest.fingerprint.converter_config_hash,
        conversion_manifest_hash: &inputs.conversion_manifest_hash,
        conversion_checkpoint_hash: &inputs.conversion_checkpoint_hash,
        catalog_hash: &actual_catalog_hash,
        catalog_metadata_hash: &conversion_catalog_metadata_hash,
        strategy: &inputs.manifest.strategy,
        run_purpose: run_purpose_label(&inputs.manifest),
        market_structure_fixture: market_structure_label(&inputs.manifest),
        fidelity_class: canonical_table.fidelity_class,
        claim_limits,
        warnings: result_contract_warnings(&nt_result),
        mechanical_blockers: Vec::new(),
        nt_result: &nt_result,
        artifact_uris: inputs.artifact_uris,
        created_at: inputs.created_at,
    })?;

    let projection = CatalogProjection {
        catalog_root: inputs.catalog_root.clone(),
        nt_instrument_id: conversion_catalog_metadata.nt_instrument_id.clone(),
        data_type: conversion_catalog_metadata.nt_data_type.clone(),
        trade_count: conversion_catalog_metadata.canonical_rows,
        catalog_hash: actual_catalog_hash,
        fidelity_class: canonical_table.fidelity_class,
    };
    let read_back_count = read_back.len();
    let mut output = BacktestRunOutput {
        canonical_table,
        projection,
        conversion_checkpoint,
        conversion_manifest,
        conversion_catalog_metadata,
        conversion_checkpoint_hash: inputs.conversion_checkpoint_hash,
        conversion_manifest_hash: inputs.conversion_manifest_hash,
        read_back_count,
        nt_result,
        contract,
    };
    redact_operator_contract(&mut output);

    fs::write(
        &inputs.proof_path,
        serde_json::to_string_pretty(&inputs.accepted_source_proof)
            .context("serialize accepted source proof")?,
    )
    .with_context(|| format!("write {}", inputs.proof_path.display()))?;
    fs::write(
        &inputs.contract_path,
        serde_json::to_string_pretty(&output.contract).context("serialize result contract")?,
    )
    .with_context(|| format!("write {}", inputs.contract_path.display()))?;
    fs::write(
        &inputs.run_manifest_path,
        serde_json::to_string_pretty(&inputs.spec_manifest.to_artifact_manifest()?)
            .context("serialize resolved run manifest")?,
    )
    .with_context(|| format!("write {}", inputs.run_manifest_path.display()))?;

    Ok(RunArtifacts {
        verified_sha256: inputs.verified_sha256,
        accepted_source_proof: inputs.accepted_source_proof,
        canonical_artifact_path: inputs.canonical_artifact_path,
        catalog_root: inputs.catalog_root,
        proof_path: inputs.proof_path,
        contract_path: inputs.contract_path,
        run_manifest_path: inputs.run_manifest_path,
        conversion_manifest_path: inputs.conversion_manifest_path,
        conversion_checkpoint_path: inputs.conversion_checkpoint_path,
        catalog_metadata_path: inputs.catalog_metadata_path,
        output,
    })
}

fn read_json_artifact<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let bytes = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    serde_json::from_slice(&bytes).with_context(|| format!("parse {}", path.display()))
}

/// Run the vertical slice from a parsed [`RunSpec`] and the raw bytes
/// of the accepted object, writing artifacts under `output_dir`.
///
/// # Errors
///
/// Returns an error if the object hash does not match the run-spec, the object
/// cannot be decoded as the configured payload container, source-proof
/// acceptance / ledger selection fails, or any backtest gate fails.
pub fn run_from_run_spec(
    spec: &RunSpec,
    object_bytes: &[u8],
    output_dir: &Path,
) -> Result<RunArtifacts> {
    validate_converter_config(&spec.converter)?;

    let object_byte_len = object_bytes.len() as u64;
    ensure!(
        object_byte_len == spec.accepted_object.bytes,
        "object byte length {object_byte_len} does not match run-spec {}",
        spec.accepted_object.bytes
    );

    ensure_object_within_raw_payload_limit(&spec.converter.raw_payload, object_byte_len)?;

    // Re-verify the accepted object content hash against the run-spec, so raw
    // staged data can never reach the backtest without matching the pinned hash.
    let mut hasher = Sha256::new();
    hasher.update(object_bytes);
    let verified_sha256 = hex::encode(hasher.finalize());
    ensure!(
        verified_sha256 == spec.accepted_object.sha256,
        "object SHA-256 {verified_sha256} does not match run-spec {}",
        spec.accepted_object.sha256
    );

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
        converter_config_hash: spec
            .converter
            .content_hash()
            .context("hash converter config")?,
    };
    let canonical_path = output_dir.join(CANONICAL_ARTIFACT_FILE);
    let catalog_root = output_dir.join(CATALOG_DIR);
    let contract_path = output_dir.join(RESULT_CONTRACT_FILE);
    let run_manifest_path = output_dir.join(BACKTEST_RUN_MANIFEST_FILE);
    let proof_path = output_dir.join(ACCEPTED_SOURCE_PROOF_FILE);
    let conversion_manifest_path =
        output_dir.join(crate::conversion_boundary::CONVERSION_MANIFEST_FILE);
    let conversion_checkpoint_path =
        output_dir.join(crate::conversion_boundary::CONVERSION_CHECKPOINT_FILE);
    let catalog_metadata_path = output_dir.join(crate::conversion_boundary::CATALOG_METADATA_FILE);
    let catalog_path = catalog_root
        .to_str()
        .context("catalog path is not valid UTF-8")?
        .to_string();

    // Bind the manifest catalog input to the local projection root.
    let contract_manifest_hash = spec.manifest.manifest_hash();
    let mut manifest = spec.manifest.clone();
    manifest.catalog_input.catalog_path = catalog_path;
    manifest.catalog_input.catalog_fs_protocol = CATALOG_FS_PROTOCOL_NONE.to_string();
    manifest.catalog_input.catalog_fs_storage_options.clear();
    manifest
        .catalog_input
        .catalog_fs_rust_storage_options
        .clear();
    let artifact_uris = portable_artifact_uris(&manifest);

    match inspect_conversion_output(output_dir, &conversion_fingerprint)? {
        ConversionOutputState::Complete {
            manifest_hash,
            checkpoint_hash,
            catalog_hash,
        } => {
            return run_from_completed_output(CompletedOutputInputs {
                verified_sha256,
                accepted_source_proof: accepted_proof,
                accepted: &accepted,
                canonical_artifact_path: canonical_path,
                catalog_root,
                proof_path,
                contract_path,
                run_manifest_path,
                conversion_manifest_path,
                conversion_checkpoint_path,
                catalog_metadata_path,
                manifest,
                contract_manifest_hash,
                conversion_manifest_hash: manifest_hash,
                conversion_checkpoint_hash: checkpoint_hash,
                expected_catalog_hash: catalog_hash,
                artifact_uris,
                created_at: &spec.created_at_utc,
                spec_manifest: &spec.manifest,
            });
        }
        ConversionOutputState::CleanNew | ConversionOutputState::ResumeFromCheckpoint { .. } => {}
    }

    fs::create_dir_all(output_dir)
        .with_context(|| format!("create output dir {}", output_dir.display()))?;
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

    // Decode to CSV text only when conversion is required. Completed outputs
    // are reused from the proven canonical Parquet artifact.
    let csv_text = decode_csv_payload(&spec.converter.raw_payload, object_bytes)?;

    let mut output = run_backtest(BacktestRunInputs {
        accepted: &accepted,
        identity: &spec.identity,
        instrument_spec: &spec.instrument_spec,
        csv_text: &csv_text,
        capture_time_nanos: rfc3339_to_nanos(&spec.capture_time_utc)?,
        manifest: &manifest,
        contract_manifest_hash: &contract_manifest_hash,
        converter: &spec.converter,
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
    fs::write(
        &run_manifest_path,
        serde_json::to_string_pretty(&spec.manifest.to_artifact_manifest()?)
            .context("serialize resolved run manifest")?,
    )
    .with_context(|| format!("write {}", run_manifest_path.display()))?;
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
        run_manifest_path,
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
    object_bytes: &[u8],
    output_dir: &Path,
) -> Result<PublishedRunArtifacts> {
    run_from_run_spec_and_publish_with_options(
        spec,
        object_bytes,
        output_dir,
        PublishOptions::default(),
    )
}

pub fn run_from_run_spec_and_publish_with_options(
    spec: &RunSpec,
    object_bytes: &[u8],
    output_dir: &Path,
    options: PublishOptions,
) -> Result<PublishedRunArtifacts> {
    let mut resolver = |_region: &str, _path: &str| {
        Err("artifact-store SSM resolver was not configured".to_string())
    };
    run_from_run_spec_and_publish_with_resolver(
        spec,
        object_bytes,
        output_dir,
        options,
        &mut resolver,
    )
}

pub fn run_from_run_spec_and_publish_with_resolver<F>(
    spec: &RunSpec,
    object_bytes: &[u8],
    output_dir: &Path,
    options: PublishOptions,
    resolver: &mut F,
) -> Result<PublishedRunArtifacts>
where
    F: FnMut(&str, &str) -> std::result::Result<String, String>,
{
    let storage_options = spec
        .manifest
        .artifact_store_storage_options_resolved(resolver)
        .map_err(|error| anyhow::anyhow!("artifact-store options rejected: {error}"))?;
    run_from_run_spec_and_publish_with_resolved_storage_options(
        spec,
        object_bytes,
        output_dir,
        options,
        storage_options.as_ref(),
    )
}

pub fn run_from_run_spec_and_publish_with_resolved_storage_options(
    spec: &RunSpec,
    object_bytes: &[u8],
    output_dir: &Path,
    options: PublishOptions,
    storage_options: Option<&BTreeMap<String, String>>,
) -> Result<PublishedRunArtifacts> {
    let mut run = run_from_run_spec(spec, object_bytes, output_dir)?;
    let mut published_artifacts = if options.prove_published_catalog {
        publish_output_artifacts_with_storage_options_excluding(
            output_dir,
            &spec.manifest.output_prefix,
            storage_options,
            &[CATALOG_METADATA_FILE, RESULT_CONTRACT_FILE],
        )?
    } else {
        publish_output_artifacts_with_storage_options(
            output_dir,
            &spec.manifest.output_prefix,
            storage_options,
        )?
    };
    let published_catalog_proof = if options.prove_published_catalog {
        let proof = prove_published_catalog_consumption(spec, &run.output, storage_options)
            .context("published catalog proof failed")?;
        let updated_paths = write_published_catalog_proof(output_dir, &mut run, &proof)?;
        published_artifacts.extend(publish_selected_artifacts_with_storage_options(
            output_dir,
            &updated_paths,
            &spec.manifest.output_prefix,
            storage_options,
        )?);
        Some(proof)
    } else {
        None
    };
    Ok(PublishedRunArtifacts {
        run,
        published_artifacts,
        published_catalog_proof,
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
    publish_output_artifacts_with_storage_options(output_dir, output_prefix, None)
}

fn publish_output_artifacts_with_storage_options(
    output_dir: &Path,
    output_prefix: &str,
    storage_options: Option<&BTreeMap<String, String>>,
) -> Result<Vec<PublishedArtifact>> {
    publish_output_artifacts_with_storage_options_excluding(
        output_dir,
        output_prefix,
        storage_options,
        &[],
    )
}

fn publish_output_artifacts_with_storage_options_excluding(
    output_dir: &Path,
    output_prefix: &str,
    storage_options: Option<&BTreeMap<String, String>>,
    excluded_relative_paths: &[&str],
) -> Result<Vec<PublishedArtifact>> {
    ensure!(
        output_dir.is_dir(),
        "output directory does not exist: {}",
        output_dir.display()
    );
    let files = collect_output_files(output_dir)?
        .into_iter()
        .filter(|path| {
            artifact_relative_path(output_dir, path)
                .map(|relative| !excluded_relative_paths.contains(&relative.as_str()))
                .unwrap_or(true)
        })
        .collect::<Vec<_>>();
    ensure!(
        !files.is_empty(),
        "output directory has no artifacts: {}",
        output_dir.display()
    );

    publish_selected_artifacts_with_storage_options(
        output_dir,
        &files,
        output_prefix,
        storage_options,
    )
}

fn publish_selected_artifacts_with_storage_options(
    output_dir: &Path,
    files: &[PathBuf],
    output_prefix: &str,
    storage_options: Option<&BTreeMap<String, String>>,
) -> Result<Vec<PublishedArtifact>> {
    ensure_local_publish_root_exists(output_prefix)?;
    let object_store_options = storage_options
        .cloned()
        .map(|options| options.into_iter().collect());
    let (object_store, base_path, _) =
        create_object_store_from_path(output_prefix, object_store_options)
            .with_context(|| format!("open output prefix {output_prefix:?}"))?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("build object-store runtime")?;
    let normalized_prefix = output_prefix.trim_end_matches('/');

    let mut targets = Vec::with_capacity(files.len());
    for local_path in files {
        let relative = artifact_relative_path(output_dir, local_path)?;
        let object_path = ObjectPath::from(published_object_key(&base_path, &relative));
        targets.push((local_path, relative, object_path));
    }
    for (_, relative, object_path) in &targets {
        match runtime.block_on(object_store.head(object_path)) {
            Ok(_) => anyhow::bail!(
                "published artifact {relative} already exists under {output_prefix}; choose a clean output_prefix"
            ),
            Err(ObjectStoreError::NotFound { .. }) => {}
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("check published artifact {relative} under {output_prefix}")
                });
            }
        }
    }

    let mut published = Vec::with_capacity(targets.len());
    for (local_path, relative, object_path) in targets {
        let bytes =
            fs::read(local_path).with_context(|| format!("read {}", local_path.display()))?;
        let byte_len = bytes.len() as u64;
        let mut hasher = Sha256::new();
        hasher.update(&bytes);
        let sha256 = hex::encode(hasher.finalize());
        runtime
            .block_on(object_store.put_opts(
                &object_path,
                Bytes::from(bytes).into(),
                PutMode::Create.into(),
            ))
            .with_context(|| format!("publish artifact {relative} to {output_prefix}"))?;
        published.push(PublishedArtifact {
            local_path: local_path.clone(),
            published_uri: format!("{normalized_prefix}/{relative}"),
            bytes: byte_len,
            sha256,
        });
    }

    Ok(published)
}

fn published_object_key(base_path: &str, relative: &str) -> String {
    if base_path.is_empty() {
        relative.to_string()
    } else {
        format!("{}/{}", base_path.trim_end_matches('/'), relative)
    }
}

fn prove_published_catalog_consumption(
    spec: &RunSpec,
    local_output: &BacktestRunOutput,
    storage_options: Option<&BTreeMap<String, String>>,
) -> Result<PublishedCatalogProof> {
    let (manifest, catalog_uri) = published_catalog_manifest(spec, storage_options)?;
    let nt_result = run_nt_backtest_node(&manifest)?;
    let expected_iterations = local_output.nt_result.iterations;
    ensure!(
        nt_result.iterations == expected_iterations,
        "published catalog BacktestNode iterations {} did not match local verified iterations {}",
        nt_result.iterations,
        expected_iterations
    );
    let direct_s3_catalog_access_proven =
        manifest.catalog_input.catalog_fs_protocol == "s3" && catalog_uri.starts_with("s3://");
    Ok(PublishedCatalogProof {
        proof_version: "published-catalog-proof.v1".to_string(),
        catalog_uri,
        catalog_fs_protocol: manifest.catalog_input.catalog_fs_protocol,
        direct_s3_catalog_access_proven,
        expected_iterations,
        nt_iterations: nt_result.iterations,
        run_config_id: nt_result.run_config_id,
        nt_version: local_output.contract.nt_version.clone(),
        created_at: spec.created_at_utc.clone(),
    })
}

fn published_catalog_manifest(
    spec: &RunSpec,
    storage_options: Option<&BTreeMap<String, String>>,
) -> Result<(BacktestingRunManifest, String)> {
    let catalog_uri = portable_artifact_uri(&spec.manifest.output_prefix, CATALOG_DIR);
    let mut manifest = spec.manifest.clone();
    manifest.catalog_input.catalog_fs_storage_options.clear();
    manifest
        .catalog_input
        .catalog_fs_rust_storage_options
        .clear();
    manifest.artifact_store.storage_options.clear();
    manifest.artifact_store.rust_storage_options.clear();
    manifest.artifact_store.ssm_parameters = None;
    if let Some(local_path) = catalog_uri.strip_prefix("file://") {
        manifest.catalog_input.catalog_path = local_path.to_string();
        manifest.catalog_input.catalog_fs_protocol = CATALOG_FS_PROTOCOL_NONE.to_string();
    } else if let Some((protocol, path)) = catalog_uri.split_once("://") {
        manifest.catalog_input.catalog_path = path.to_string();
        manifest.catalog_input.catalog_fs_protocol = protocol.to_string();
        manifest.catalog_input.catalog_fs_rust_storage_options =
            storage_options.cloned().unwrap_or_default();
    } else {
        manifest.catalog_input.catalog_path = catalog_uri.clone();
        manifest.catalog_input.catalog_fs_protocol = CATALOG_FS_PROTOCOL_NONE.to_string();
    }
    Ok((manifest, catalog_uri))
}

fn write_published_catalog_proof(
    output_dir: &Path,
    run: &mut RunArtifacts,
    proof: &PublishedCatalogProof,
) -> Result<Vec<PathBuf>> {
    let proof_path = output_dir.join(PUBLISHED_CATALOG_PROOF_FILE);
    fs::write(
        &proof_path,
        serde_json::to_string_pretty(proof).context("serialize published catalog proof")?,
    )
    .with_context(|| format!("write {}", proof_path.display()))?;

    run.output.conversion_catalog_metadata = run
        .output
        .conversion_catalog_metadata
        .clone()
        .with_execution_catalog_access(
            proof.catalog_uri.clone(),
            proof.direct_s3_catalog_access_proven,
        );
    write_completed_conversion_artifacts(
        output_dir,
        &run.output.conversion_manifest,
        &run.output.conversion_checkpoint,
        &run.output.conversion_catalog_metadata,
    )?;
    run.output.contract.catalog_metadata_hash = run
        .output
        .conversion_catalog_metadata
        .content_hash()
        .context("hash updated catalog metadata")?;
    run.output
        .contract
        .validate()
        .map_err(|error| anyhow::anyhow!("updated result contract rejected: {error}"))?;
    fs::write(
        &run.contract_path,
        serde_json::to_string_pretty(&run.output.contract)
            .context("serialize updated result contract")?,
    )
    .with_context(|| format!("write {}", run.contract_path.display()))?;

    Ok(vec![
        proof_path,
        output_dir.join(CATALOG_METADATA_FILE),
        run.contract_path.clone(),
    ])
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
    use std::io::{Cursor, Write};

    use flate2::{Compression, write::GzEncoder};

    use super::*;
    use crate::canonical_trades::{CsvTimestampUnit, RawPayloadConfig, RawPayloadContainer};
    use crate::conversion_boundary::{
        CATALOG_METADATA_FILE, CONVERSION_CHECKPOINT_FILE, CONVERSION_MANIFEST_FILE,
        ConversionCatalogMetadata, ConversionCheckpoint, ConversionManifest,
    };
    use crate::result_contract::BacktestResultContract;
    use crate::run_manifest::{BacktestRunManifestArtifact, NtSurfaceClassification};
    use crate::source_proof::EvidenceState;

    const COMMITTED_RUN_SPEC: &str = include_str!(
        "../../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-run-spec.bnbusdc-2026-03-01.toml"
    );
    const COMMITTED_RESULT_CONTRACT: &str = include_str!(
        "../../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-result-contract.bnbusdc-2026-03-01.json"
    );
    const COMMITTED_CATALOG_METADATA: &str = include_str!(
        "../../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-catalog-metadata.bnbusdc-2026-03-01.json"
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
    const ALT_SCHEMA_CSV: &str = "trade_id,ts_ms,px,qty,taker_side,ignored\n\
        a1,1772323201665,617.2,0.3,B,0\n\
        a2,1772323312219,617.9,0.1456,S,0\n";
    const BINANCE_HEADERLESS_CSV: &str = "101735393,617.34000000,1.61900000,999.47346000,1772323201711256,True,True\n\
        101735394,617.34000000,0.07200000,44.44848000,1772323201815330,False,True\n";

    fn gzip(text: &str) -> Vec<u8> {
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(text.as_bytes()).expect("gzip write");
        encoder.finish().expect("gzip finish")
    }

    fn zip_single_csv(member_name: &str, text: &str) -> Vec<u8> {
        let cursor = Cursor::new(Vec::new());
        let mut writer = zip::ZipWriter::new(cursor);
        writer
            .start_file(member_name, zip::write::FileOptions::default())
            .expect("start zip member");
        writer.write_all(text.as_bytes()).expect("write zip member");
        writer.finish().expect("finish zip").into_inner()
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
        spec.converter.raw_payload = RawPayloadConfig {
            container: RawPayloadContainer::CsvGzip,
            max_object_bytes: gz_bytes.len() as u64,
            max_decoded_bytes: 4096,
            zip_member: None,
        };
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
        assert!(
            artifacts.run_manifest_path.exists(),
            "resolved run manifest written"
        );
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
    fn run_from_run_spec_writes_resolved_run_manifest_artifact() {
        let gz = gzip(SAMPLE_CSV);
        let spec = run_spec_for(&gz);
        let dir = tempfile::TempDir::new().unwrap();
        let artifacts = run_from_run_spec(&spec, &gz, dir.path()).expect("operator run");

        let manifest_json = fs::read_to_string(&artifacts.run_manifest_path).unwrap();
        let parsed: BacktestRunManifestArtifact = serde_json::from_str(&manifest_json).unwrap();

        assert_eq!(
            parsed.submitted_manifest_hash,
            spec.manifest.manifest_hash()
        );
        assert_eq!(parsed.manifest, spec.manifest);
        assert!(
            parsed.resolved_nt_surfaces.iter().any(|surface| {
                surface.classification == NtSurfaceClassification::Defaulted
                    && surface.surface == "run.chunk_size"
                    && surface.resolved_value == "None"
            }),
            "{:?}",
            parsed.resolved_nt_surfaces
        );
        assert!(
            parsed.resolved_nt_surfaces.iter().any(|surface| {
                surface.classification == NtSurfaceClassification::PassThrough
                    && surface.surface == "run.id"
                    && surface.resolved_value == spec.manifest.run_id
            }),
            "{:?}",
            parsed.resolved_nt_surfaces
        );
        assert!(
            parsed.resolved_nt_surfaces.iter().any(|surface| {
                surface.classification == NtSurfaceClassification::UnsupportedForNow
                    && surface.surface == "venue.fill_model"
            }),
            "{:?}",
            parsed.resolved_nt_surfaces
        );
        assert!(
            !manifest_json.contains(dir.path().to_str().unwrap()),
            "published manifest artifact must not carry local execution paths"
        );
    }

    #[test]
    fn run_from_run_spec_rejects_object_byte_count_mismatch_before_artifacts() {
        let gz = gzip(SAMPLE_CSV);
        let mut spec = run_spec_for(&gz);
        spec.accepted_object.bytes += 1;
        let dir = tempfile::TempDir::new().unwrap();

        let err = match run_from_run_spec(&spec, &gz, dir.path()) {
            Ok(_) => panic!("object bytes must match the run-spec before conversion"),
            Err(err) => err,
        };

        assert!(err.to_string().contains("object byte length"), "{err}");
        assert!(
            !dir.path().join(CONVERSION_CHECKPOINT_FILE).exists(),
            "byte-count rejection must happen before conversion checkpoint writes"
        );
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
            &parsed.artifact_uris.catalog_metadata_uri,
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
        let converter_config_hash = spec.converter.content_hash().unwrap();
        let manifest_hash = manifest.content_hash().unwrap();
        let checkpoint_hash = checkpoint.content_hash().unwrap();
        let metadata_hash = metadata.content_hash().unwrap();

        assert_eq!(
            manifest.fingerprint.converter_config_hash,
            converter_config_hash
        );
        assert_eq!(
            checkpoint.fingerprint.converter_config_hash,
            converter_config_hash
        );
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
        assert_eq!(
            artifacts.output.contract.converter_config_hash,
            converter_config_hash
        );
        assert_eq!(
            artifacts.output.contract.catalog_metadata_hash,
            metadata_hash
        );
        assert_eq!(
            artifacts.output.contract.artifact_uris.catalog_metadata_uri,
            format!(
                "{}/{}",
                spec.manifest.output_prefix.trim_end_matches('/'),
                CATALOG_METADATA_FILE
            )
        );
    }

    #[test]
    fn run_from_run_spec_rejects_tampered_object() {
        // The committed run-spec pins the real (uncommitted) object hash; feeding
        // it the synthetic bytes must trip the SHA-256 re-verification.
        let gz = gzip(SAMPLE_CSV);
        let mut spec: RunSpec = toml::from_str(COMMITTED_RUN_SPEC).expect("parse");
        spec.accepted_object.bytes = gz.len() as u64;
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
    fn run_from_run_spec_accepts_completed_output_on_second_run() {
        let gz = gzip(SAMPLE_CSV);
        let spec = run_spec_for(&gz);
        let dir = tempfile::TempDir::new().unwrap();
        run_from_run_spec(&spec, &gz, dir.path()).expect("first run");
        run_from_run_spec(&spec, &gz, dir.path()).expect("second run accepts completed output");
    }

    #[test]
    fn run_from_run_spec_reuses_completed_output_without_rebuilding_catalog() {
        let gz = gzip(SAMPLE_CSV);
        let spec = run_spec_for(&gz);
        let dir = tempfile::TempDir::new().unwrap();
        let first = run_from_run_spec(&spec, &gz, dir.path()).expect("first run");
        let checkpoint_json =
            fs::read_to_string(&first.conversion_checkpoint_path).expect("checkpoint");
        let manifest_json = fs::read_to_string(&first.conversion_manifest_path).expect("manifest");
        let metadata_json = fs::read_to_string(&first.catalog_metadata_path).expect("metadata");
        let catalog_hash = first.output.projection.catalog_hash.clone();
        let read_back_count = first.output.read_back_count;

        let catalog_marker = first.catalog_root.join(".reuse-sentinel");
        fs::write(&catalog_marker, b"must survive completed-output reuse").expect("marker");

        let second = run_from_run_spec(&spec, &gz, dir.path()).expect("second run");

        assert!(
            catalog_marker.exists(),
            "completed conversion output must be reused without deleting the NT catalog root"
        );
        assert_eq!(
            fs::read_to_string(&second.conversion_checkpoint_path).expect("checkpoint"),
            checkpoint_json
        );
        assert_eq!(
            fs::read_to_string(&second.conversion_manifest_path).expect("manifest"),
            manifest_json
        );
        assert_eq!(
            fs::read_to_string(&second.catalog_metadata_path).expect("metadata"),
            metadata_json
        );
        assert_eq!(second.output.projection.catalog_hash, catalog_hash);
        assert_eq!(second.output.read_back_count, read_back_count);
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
        assert!(
            published
                .published_artifacts
                .iter()
                .any(|artifact| artifact.published_uri.ends_with(BACKTEST_RUN_MANIFEST_FILE)),
            "publish set must include the artifact-local run manifest"
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
    fn publish_output_artifacts_rejects_existing_published_artifact_without_overwrite() {
        let output_dir = tempfile::TempDir::new().unwrap();
        fs::write(
            output_dir.path().join("result-contract.json"),
            b"new-result",
        )
        .unwrap();
        let published_root = tempfile::TempDir::new().unwrap();
        let output_prefix = format!(
            "file://{}/backtests/published-run",
            published_root.path().display()
        );
        let existing = published_root
            .path()
            .join("backtests/published-run/result-contract.json");
        fs::create_dir_all(existing.parent().unwrap()).unwrap();
        fs::write(&existing, b"existing-result").unwrap();

        let err = publish_output_artifacts(output_dir.path(), &output_prefix)
            .err()
            .expect("publish must reject pre-existing artifact");

        assert!(err.to_string().contains("already exists"), "{err}");
        assert_eq!(
            fs::read(&existing).unwrap(),
            b"existing-result",
            "existing published artifact must not be overwritten"
        );
    }

    #[test]
    fn run_from_run_spec_and_publish_can_prove_published_catalog_consumption() {
        let gz = gzip(SAMPLE_CSV);
        let mut spec = run_spec_for(&gz);
        let local_dir = tempfile::TempDir::new().unwrap();
        let published_root = tempfile::TempDir::new().unwrap();
        let artifact_root = format!("file://{}", published_root.path().display());
        spec.manifest.artifact_root = artifact_root.clone();
        spec.manifest.output_prefix = format!("{artifact_root}/backtests/published-run");

        let published = run_from_run_spec_and_publish_with_options(
            &spec,
            &gz,
            local_dir.path(),
            PublishOptions {
                prove_published_catalog: true,
            },
        )
        .expect("published run with catalog proof");

        let proof = published
            .published_catalog_proof
            .expect("published catalog proof");
        assert_eq!(proof.nt_iterations, 3);
        assert_eq!(proof.expected_iterations, 3);
        assert_eq!(
            proof.catalog_uri,
            format!("{}/{}", spec.manifest.output_prefix, CATALOG_DIR)
        );
        assert!(
            !proof.direct_s3_catalog_access_proven,
            "file publish proof must not claim direct S3"
        );
        assert!(
            published_root
                .path()
                .join("backtests/published-run/published-catalog-proof.json")
                .is_file(),
            "published proof artifact must be copied after the direct catalog run"
        );
    }

    #[test]
    fn run_from_run_spec_and_publish_rejects_s3_without_ssm_before_running_backtest() {
        let gz = gzip(SAMPLE_CSV);
        let mut spec = run_spec_for(&gz);
        let local_dir = tempfile::TempDir::new().unwrap();
        spec.manifest.output_prefix = "s3://example-bucket/backtests/published-run".to_string();
        spec.manifest.artifact_store.rust_storage_options =
            BTreeMap::from([("region".to_string(), "us-east-1".to_string())]);

        let err = match run_from_run_spec_and_publish_with_options(
            &spec,
            &gz,
            local_dir.path(),
            PublishOptions {
                prove_published_catalog: true,
            },
        ) {
            Ok(_) => panic!("publish must reject missing SSM credential binding"),
            Err(error) => error,
        };

        assert!(
            err.to_string()
                .contains("artifact_store.ssm_parameters must resolve"),
            "publish must fail on missing SSM credential binding: {err}"
        );
        assert!(
            !local_dir.path().join(CONVERSION_MANIFEST_FILE).exists(),
            "publish credential validation must happen before local conversion/backtest artifacts are written"
        );
    }

    #[test]
    fn published_catalog_manifest_uses_resolved_artifact_store_options() {
        let gz = gzip(SAMPLE_CSV);
        let mut spec = run_spec_for(&gz);
        spec.manifest.output_prefix = "s3://example-bucket/backtests/published-run".to_string();
        spec.manifest.artifact_store.rust_storage_options =
            BTreeMap::from([("region".to_string(), "us-east-1".to_string())]);
        spec.manifest.artifact_store.ssm_parameters =
            Some(crate::run_manifest::ManifestArtifactStoreSsmParameters {
                region: "us-east-1".to_string(),
                access_key_id: "/bolt/artifacts/access-key-id".to_string(),
                secret_access_key: "/bolt/artifacts/secret-access-key".to_string(),
                session_token: None,
            });
        let resolved_options = BTreeMap::from([
            ("region".to_string(), "us-east-1".to_string()),
            ("access_key_id".to_string(), "AKIATEST".to_string()),
            ("secret_access_key".to_string(), "secret-value".to_string()),
        ]);

        let (manifest, catalog_uri) =
            published_catalog_manifest(&spec, Some(&resolved_options)).expect("manifest");

        assert_eq!(
            catalog_uri,
            "s3://example-bucket/backtests/published-run/nt-catalog"
        );
        assert_eq!(manifest.catalog_input.catalog_fs_protocol, "s3");
        assert_eq!(
            manifest.catalog_input.catalog_path,
            "example-bucket/backtests/published-run/nt-catalog"
        );
        assert!(manifest.catalog_input.catalog_fs_storage_options.is_empty());
        assert_eq!(
            manifest.catalog_input.catalog_fs_rust_storage_options,
            resolved_options
        );
        assert!(
            !serde_json::to_string(&manifest)
                .expect("serialize manifest")
                .contains("/bolt/artifacts"),
            "published catalog manifest must not carry SSM parameter paths into NT"
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
    fn committed_result_contract_converter_config_hash_matches_run_spec() {
        let spec: RunSpec = toml::from_str(COMMITTED_RUN_SPEC).expect("run-spec parses");
        let contract: BacktestResultContract =
            serde_json::from_str(COMMITTED_RESULT_CONTRACT).expect("result contract parses");

        assert_eq!(
            contract.converter_config_hash,
            spec.converter
                .content_hash()
                .expect("converter config hash")
        );
    }

    #[test]
    fn run_from_run_spec_rejects_unregistered_converter_version() {
        let gz = gzip(SAMPLE_CSV);
        let mut spec = run_spec_for(&gz);
        spec.converter.version = "2".to_string();
        let dir = tempfile::TempDir::new().unwrap();

        let err = run_from_run_spec(&spec, &gz, dir.path())
            .err()
            .expect("unregistered converter version must be rejected");

        assert!(err.to_string().contains("registered converter"), "{err}");
    }

    #[test]
    fn run_from_run_spec_uses_configured_csv_trade_mapping() {
        let gz = gzip(ALT_SCHEMA_CSV);
        let mut spec = run_spec_for(&gz);
        spec.accepted_object.schema_columns =
            ["trade_id", "ts_ms", "px", "qty", "taker_side", "ignored"]
                .map(str::to_string)
                .to_vec();
        spec.converter.csv.trade_id_column = "trade_id".to_string();
        spec.converter.csv.timestamp_column = "ts_ms".to_string();
        spec.converter.csv.price_column = "px".to_string();
        spec.converter.csv.size_column = "qty".to_string();
        spec.converter.csv.side_column = "taker_side".to_string();
        spec.converter.csv.buyer_side_values = vec!["B".to_string()];
        spec.converter.csv.seller_side_values = vec!["S".to_string()];
        let dir = tempfile::TempDir::new().unwrap();

        let artifacts = run_from_run_spec(&spec, &gz, dir.path()).expect("operator run");

        assert_eq!(artifacts.output.canonical_table.rows.len(), 2);
        assert_eq!(artifacts.output.canonical_table.rows[0].trade_id, "a1");
        assert_eq!(
            artifacts.output.canonical_table.rows[0].aggressor_side,
            "BUYER"
        );
        assert_eq!(
            artifacts.output.canonical_table.rows[1].aggressor_side,
            "SELLER"
        );
    }

    #[test]
    fn run_from_run_spec_rejects_object_above_configured_payload_max_before_artifacts() {
        let gz = gzip(SAMPLE_CSV);
        let mut spec = run_spec_for(&gz);
        spec.converter.raw_payload.max_object_bytes = gz.len() as u64 - 1;
        let dir = tempfile::TempDir::new().unwrap();

        let err = run_from_run_spec(&spec, &gz, dir.path())
            .err()
            .expect("configured object max must reject oversized object");

        assert!(
            err.to_string()
                .contains("converter.raw_payload.max_object_bytes"),
            "{err}"
        );
        assert!(
            !dir.path().join(CONVERSION_CHECKPOINT_FILE).exists(),
            "object max rejection must happen before checkpoint writes"
        );
    }

    #[test]
    fn run_from_run_spec_rejects_decoded_payload_above_configured_max_before_catalog_work() {
        let gz = gzip(SAMPLE_CSV);
        let mut spec = run_spec_for(&gz);
        spec.converter.raw_payload.max_decoded_bytes = 1;
        let dir = tempfile::TempDir::new().unwrap();

        let err = run_from_run_spec(&spec, &gz, dir.path())
            .err()
            .expect("configured decoded max must reject oversized CSV expansion");

        assert!(
            err.to_string()
                .contains("converter.raw_payload.max_decoded_bytes"),
            "{err}"
        );
        assert!(
            !dir.path().join(CATALOG_DIR).exists(),
            "decoded max rejection must happen before NT catalog projection"
        );
        assert!(
            !dir.path().join(CANONICAL_ARTIFACT_FILE).exists(),
            "decoded max rejection must happen before canonical artifact writes"
        );
    }

    #[test]
    fn run_from_run_spec_uses_configured_single_csv_zip_payload() {
        let zip_bytes = zip_single_csv("BNBUSDC-trades-2026-03-01.csv", BINANCE_HEADERLESS_CSV);
        let mut spec = run_spec_for(&zip_bytes);
        spec.accepted_object.s3_uri =
            "s3://bolt-parquet/backfill-staging/binance/BNBUSDC-trades-2026-03-01.zip".to_string();
        spec.accepted_object.source_url =
            "https://data.binance.vision/data/spot/daily/trades/BNBUSDC/BNBUSDC-trades-2026-03-01.zip"
                .to_string();
        spec.accepted_object.schema_columns = [
            "trade_id",
            "price",
            "qty",
            "quote_qty",
            "time",
            "is_buyer_maker",
            "is_best_match",
        ]
        .into_iter()
        .map(ToString::to_string)
        .collect();
        spec.source_proof.source_binding = "binance-spot-native-trades".to_string();
        spec.source_proof.venue = "binance".to_string();
        spec.source_proof.evidence_state = EvidenceState::DirectlyBackfillable;
        spec.source_proof.source_proof_id = "source-proof-binance-spot-native-trades".to_string();
        spec.source_proof.raw_sample_uri = spec.accepted_object.s3_uri.clone();
        spec.manifest.venue_binding_key = "binance-spot-native-trades".to_string();
        spec.manifest.source_proof_id = "source-proof-binance-spot-native-trades".to_string();
        spec.manifest.venue.nt_venue = "BINANCE".to_string();
        spec.manifest.catalog_input.nt_instrument_id = "BNBUSDC.BINANCE".to_string();
        spec.instrument_spec.nt_instrument_id = "BNBUSDC.BINANCE".to_string();
        spec.instrument_spec.price_increment = "0.00000001".to_string();
        spec.instrument_spec.size_increment = "0.000001".to_string();
        spec.identity.nt_instrument_id = "BNBUSDC.BINANCE".to_string();
        spec.manifest.strategy.parameters.insert(
            "bar_type".to_string(),
            "BNBUSDC.BINANCE-1-MINUTE-LAST-INTERNAL".to_string(),
        );
        spec.converter.raw_payload = RawPayloadConfig {
            container: RawPayloadContainer::SingleCsvZip,
            max_object_bytes: zip_bytes.len() as u64,
            max_decoded_bytes: BINANCE_HEADERLESS_CSV.len() as u64,
            zip_member: Some("BNBUSDC-trades-2026-03-01.csv".to_string()),
        };
        spec.converter.csv.has_headers = false;
        spec.converter.csv.trade_id_column = "trade_id".to_string();
        spec.converter.csv.timestamp_column = "time".to_string();
        spec.converter.csv.timestamp_unit = CsvTimestampUnit::Microseconds;
        spec.converter.csv.price_column = "price".to_string();
        spec.converter.csv.size_column = "qty".to_string();
        spec.converter.csv.side_column = "is_buyer_maker".to_string();
        spec.converter.csv.buyer_side_values = vec!["False".to_string()];
        spec.converter.csv.seller_side_values = vec!["True".to_string()];
        let dir = tempfile::TempDir::new().unwrap();

        let artifacts = run_from_run_spec(&spec, &zip_bytes, dir.path()).expect("operator run");

        assert_eq!(artifacts.output.canonical_table.rows.len(), 2);
        assert_eq!(
            artifacts.output.canonical_table.rows[0].trade_id,
            "101735393"
        );
        assert_eq!(
            artifacts.output.canonical_table.rows[1].aggressor_side,
            "BUYER"
        );
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
    fn committed_result_contract_manifest_hash_matches_run_spec() {
        let spec: RunSpec = toml::from_str(COMMITTED_RUN_SPEC).expect("run-spec parses");
        let contract: BacktestResultContract =
            serde_json::from_str(COMMITTED_RESULT_CONTRACT).expect("result contract parses");
        assert_eq!(contract.manifest_hash, spec.manifest.manifest_hash());
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
            &contract.artifact_uris.catalog_metadata_uri,
            &contract.artifact_uris.result_contract_uri,
        ] {
            assert!(!uri.starts_with("/private/tmp/"), "{uri}");
        }
        assert!(contract.claim_limits.iter().any(|limit| {
            limit.contains("operator-attested") && limit.contains("not reproduced in CI")
        }));
    }

    #[test]
    fn committed_result_contract_binds_catalog_metadata() {
        let contract: BacktestResultContract =
            serde_json::from_str(COMMITTED_RESULT_CONTRACT).expect("result contract parses");
        let metadata: ConversionCatalogMetadata =
            serde_json::from_str(COMMITTED_CATALOG_METADATA).expect("catalog metadata parses");
        assert_eq!(
            contract.catalog_metadata_hash,
            metadata.content_hash().unwrap()
        );
        assert_eq!(
            contract.artifact_uris.catalog_metadata_uri,
            "reference://backtesting-vertical-slice/bnbusdc-2026-03-01/catalog-metadata.json"
        );
        assert!(
            !metadata.direct_s3_catalog_access_proven,
            "reference artifact must not claim direct S3 execution"
        );
    }

    #[test]
    fn committed_result_contract_records_nt_extension_surface_claim_limits() {
        let contract: BacktestResultContract =
            serde_json::from_str(COMMITTED_RESULT_CONTRACT).expect("result contract parses");
        assert!(contract.claim_limits.iter().any(|limit| {
            limit.contains("NT defaulted surface run.chunk_size")
                && limit.contains("resolved_value=None")
        }));
        assert!(contract.claim_limits.iter().any(|limit| {
            limit.contains("NT pass_through surface run.id")
                && limit.contains("backtesting-vertical-slice-bnbusdc-2026-03-01")
        }));
        assert!(
            contract
                .claim_limits
                .iter()
                .any(|limit| { limit.contains("NT unsupported_for_now surface venue.fill_model") })
        );
    }

    #[test]
    fn committed_accepted_proof_deserializes() {
        let proof: SourceProofReport =
            serde_json::from_str(COMMITTED_ACCEPTED_PROOF).expect("accepted proof parses");
        assert!(proof.is_accepted(), "committed proof is accepted");
    }
}
