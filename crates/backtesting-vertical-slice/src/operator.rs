//! Operator entrypoint glue, lifted out of `main` so it is unit-testable.
//!
//! Everything that identifies the dataset (accepted object, source proof, run
//! manifest, instrument spec) comes from a config-driven [`RunSpec`]; the only
//! runtime inputs are the raw bytes of the accepted object and an
//! output directory. [`run_from_run_spec`] re-verifies the object SHA-256
//! against the run-spec before any normalization, decodes the object,
//! verifies the already accepted source proof and binds the object through the
//! ledger, guarantees a clean catalog root, runs the backtest, and writes the
//! accepted proof, artifact-local run manifest, and result contract as JSON
//! artifacts.

use std::{
    collections::BTreeMap,
    fs,
    io::{Cursor, Read},
    path::{Component, Path, PathBuf},
};

use anyhow::{Context, Result, ensure};
use bytes::Bytes;
use nautilus_backtest::result::BacktestResult;
use nautilus_persistence::parquet::create_object_store_from_path;
use object_store::{
    Error as ObjectStoreError, ObjectStore, ObjectStoreExt, PutMode, path::Path as ObjectPath,
};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::atomic_artifact_write::atomic_write;
use crate::{
    artifact_store::{
        ArtifactStoreConfig, CatalogDispatchConfig, CreateOnlyArtifactWriter,
        CreateOnlyProbeTranscript, PersistedCatalogProjection, PersistedCatalogProjectionObject,
        ResolvedArtifactRoot, S3ConditionalPutMode,
        persist_catalog_projection_for_source_binding_guarded,
    },
    canonical_market_data::{
        CanonicalBarsTable, CanonicalFundingRatesTable, CanonicalIndexPricesTable,
        CanonicalMarkPricesTable, CanonicalOrderBookDeltasTable, CanonicalQuotesTable,
    },
    canonical_trades::{
        BAR_TABLE_FAMILY, CanonicalInstrumentIdentity, CanonicalTradesTable, ConverterConfig,
        DELTAS_TABLE_FAMILY, FUNDING_RATES_TABLE_FAMILY, INDEX_PRICES_TABLE_FAMILY,
        MARK_PRICES_TABLE_FAMILY, QUOTE_TABLE_FAMILY, RawPayloadConfig, RawPayloadContainer,
        SourceAdapterKind, TRADE_TABLE_FAMILY, normalize_registered_bar_converter,
        normalize_registered_event_stream_delta_converter, normalize_registered_funding_converter,
        normalize_registered_index_converter,
        normalize_registered_jsonl_multi_interval_bar_converter,
        normalize_registered_mark_converter, normalize_registered_order_book_delta_converter,
        normalize_registered_paged_json_bar_converter, normalize_registered_quote_converter,
        normalize_registered_seeded_l2_quote_converter,
        normalize_registered_tar_order_book_delta_converter,
        normalize_registered_tar_seeded_l2_quote_converter, require_registered_source_adapter,
        require_registered_source_adapter_for_table_family,
    },
    catalog_projection::{
        CatalogInstrumentSpec, CatalogProjection, NT_DATA_TYPE_BAR,
        NT_DATA_TYPE_FUNDING_RATE_UPDATE, NT_DATA_TYPE_INDEX_PRICE_UPDATE,
        NT_DATA_TYPE_MARK_PRICE_UPDATE, NT_DATA_TYPE_ORDER_BOOK_DELTA, NT_DATA_TYPE_QUOTE_TICK,
        NT_DATA_TYPE_TRADE_TICK, actual_nt_market_data_metadata, logical_catalog_hash,
        project_canonical_bars_to_catalog, project_canonical_funding_rates_to_catalog,
        project_canonical_index_to_catalog, project_canonical_mark_to_catalog,
        project_canonical_order_book_deltas_to_catalog, project_canonical_quotes_to_catalog,
        project_canonical_trades_to_catalog, projected_nt_market_data_row_groups, read_back_bars,
        read_back_funding_rates, read_back_index, read_back_mark, read_back_order_book_deltas,
        read_back_quotes, read_back_trade_ticks, ts_init_nanos,
    },
    conversion_boundary::{
        CATALOG_METADATA_FILE, CONVERSION_CHECKPOINT_FILE, CONVERSION_TABLES_FILE,
        ConversionCatalogMetadata, ConversionCheckpoint, ConversionCheckpointStage,
        ConversionFingerprint, ConversionManifest, ConversionOutputState, ConversionTableRecord,
        inspect_conversion_output, validate_conversion_tables_index,
        write_completed_conversion_artifacts_guarded, write_conversion_checkpoint,
        write_conversion_tables_index, write_pending_conversion_artifacts,
    },
    nt_catalog_capability::{
        NtCatalogCapabilityEvidence, NtCatalogCapabilityPlan, NtCatalogCapabilityProofArtifact,
        NtCatalogCapabilityRunSpec,
    },
    operator_work_budget::{
        OperatorWorkBudgetCommitPermit, OperatorWorkBudgetGuard, OperatorWorkBudgetStage,
    },
    result_contract::{
        BacktestResultContract, ResultArtifactUris, ResultContractInputs, build_result_contract,
    },
    run_manifest::{BacktestingRunManifest, CATALOG_FS_PROTOCOL_NONE, ManifestCatalogInput},
    runner::{
        BacktestRunInputs, BacktestRunOutput, assert_bar_read_back_matches,
        assert_delta_read_back_matches, assert_funding_read_back_matches,
        assert_index_read_back_matches, assert_mark_read_back_matches,
        assert_quote_read_back_matches, assert_read_back_matches, assert_time_window_overlaps_data,
        expected_iterations, iterations_mismatch, market_structure_label,
        nt_extension_surface_claim_limits, result_contract_feed_labels, result_contract_warnings,
        run_backtest, run_nt_backtest_node_guarded, run_purpose_label,
        time_window_excludes_all_data, window_bound_nanos,
    },
    source_proof::{
        AcceptedDataset, IngestManifestObjectRecord, SourceBindingRegistry,
        SourceProofFidelityClass, SourceProofReport, read_source_binding_registry_from_path,
        select_accepted_dataset_with_registry,
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

const OPERATOR_ATTESTED_REDACTED: &str = "operator-attested-redacted";
const OPERATOR_ATTESTED_ELAPSED_TIME_SECS: f64 = 0.0;

/// Config-driven dataset facts for one operator run.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunSpec {
    /// Ingest capture timestamp (RFC 3339).
    pub capture_time_utc: String,
    /// Result-contract `created_at` timestamp (RFC 3339).
    pub created_at_utc: String,
    /// Operator/actor recorded as accepting the source proof.
    pub accepted_by: String,
    /// Acceptance timestamp (RFC 3339).
    pub accepted_at_utc: String,
    /// Runtime source-binding registry TOML used for source-proof acceptance.
    pub source_bindings_path: PathBuf,
    pub accepted_object: IngestManifestObjectRecord,
    pub source_proof: SourceProofReport,
    pub instrument_spec: RunSpecInstrumentSpecs,
    pub identity: RunSpecInstrumentIdentities,
    pub converter: ConverterConfig,
    pub manifest: BacktestingRunManifest,
    #[serde(default)]
    pub artifact_store: Option<ArtifactStoreConfig>,
    #[serde(default)]
    pub catalog_dispatch: Option<CatalogDispatchConfig>,
    #[serde(default)]
    pub create_only_probe_id: Option<String>,
    #[serde(default)]
    pub nt_catalog_capability_proof: Option<NtCatalogCapabilityRunSpec>,
    /// Selector provenance hashes required for L2 replay result contracts.
    /// Only valid on run-specs whose accepted data is `L2_REPLAY`.
    #[serde(default)]
    pub selector_provenance: Option<RunSpecSelectorProvenance>,
}

impl RunSpec {
    /// # Errors
    ///
    /// Returns an error when the run-spec omits the durable artifact store
    /// configuration required by the publish/proof path.
    pub fn required_artifact_store(&self) -> Result<&ArtifactStoreConfig> {
        self.artifact_store
            .as_ref()
            .context("run spec missing [artifact_store] required for artifact-store publish path")
    }

    /// # Errors
    ///
    /// Returns an error when the manifest publish store and durable artifact
    /// store disagree on the physical S3 store they both describe.
    pub fn validate_artifact_store_publish_config(
        &self,
        artifact_store: &ArtifactStoreConfig,
    ) -> Result<()> {
        let manifest_root = self.manifest.artifact_root.trim_end_matches('/');
        let artifact_root = artifact_store.artifact_root.trim_end_matches('/');
        ensure!(
            manifest_root == artifact_root,
            "run spec artifact-store root mismatch: manifest.artifact_root {:?} != artifact_store.artifact_root {:?}",
            self.manifest.artifact_root,
            artifact_store.artifact_root
        );

        let output_prefix = format!("{}/", self.manifest.output_prefix.trim_end_matches('/'));
        let backtests_prefix = format!(
            "{}/{}/",
            artifact_root,
            artifact_store.subpaths.backtests.trim_matches('/')
        );
        ensure!(
            output_prefix.starts_with(&backtests_prefix),
            "run spec artifact-store output_prefix mismatch: manifest.output_prefix {:?} must be under {:?}",
            self.manifest.output_prefix,
            backtests_prefix
        );

        manifest_artifact_store_option(&self.manifest, "region").context(
            "run spec manifest.artifact_store missing region for durable artifact store",
        )?;
        for (field, value) in manifest_artifact_store_options(&self.manifest, "region") {
            ensure!(
                value == artifact_store.s3.region,
                "run spec artifact-store region mismatch: {field} {:?} != artifact_store.s3.region {:?}",
                value,
                artifact_store.s3.region
            );
        }
        if let Some(ssm_parameters) = &self.manifest.artifact_store.ssm_parameters {
            ensure!(
                ssm_parameters.region == artifact_store.s3.region,
                "run spec artifact-store SSM region mismatch: manifest.artifact_store.ssm_parameters.region {:?} != artifact_store.s3.region {:?}",
                ssm_parameters.region,
                artifact_store.s3.region
            );
        }

        let expected_conditional_put = match artifact_store.s3.conditional_put {
            S3ConditionalPutMode::Etag => "etag",
        };
        manifest_artifact_store_option(&self.manifest, "conditional_put").context(
            "run spec manifest.artifact_store missing conditional_put for durable artifact store",
        )?;
        for (field, value) in manifest_artifact_store_options(&self.manifest, "conditional_put") {
            ensure!(
                value == expected_conditional_put,
                "run spec artifact-store conditional_put mismatch: {field} {:?} != artifact_store.s3.conditional_put {:?}",
                value,
                expected_conditional_put
            );
        }
        Ok(())
    }

    /// # Errors
    ///
    /// Returns an error when the run-spec omits source-binding catalog dispatch
    /// configuration required by the publish/proof path.
    pub fn required_catalog_dispatch(&self) -> Result<&CatalogDispatchConfig> {
        self.catalog_dispatch.as_ref().context(
            "run spec missing [[catalog_dispatch.bindings]] required for artifact-store publish path",
        )
    }

    /// # Errors
    ///
    /// Returns an error when the run-spec omits the create-only probe id required
    /// by the publish/proof path.
    pub fn required_create_only_probe_id(&self) -> Result<&str> {
        self.create_only_probe_id.as_deref().context(
            "run spec missing create_only_probe_id required for artifact-store publish path",
        )
    }

    /// # Errors
    ///
    /// Returns an error when the run-spec omits the synthetic NT catalog
    /// capability proof required by the publish/proof path.
    pub fn required_nt_catalog_capability_proof(&self) -> Result<&NtCatalogCapabilityRunSpec> {
        self.nt_catalog_capability_proof.as_ref().context(
            "run spec missing [nt_catalog_capability_proof] required for artifact-store publish path",
        )
    }
}

fn manifest_artifact_store_option<'a>(
    manifest: &'a BacktestingRunManifest,
    key: &str,
) -> Option<&'a str> {
    manifest
        .artifact_store
        .rust_storage_options
        .get(key)
        .or_else(|| manifest.artifact_store.storage_options.get(key))
        .map(String::as_str)
}

fn manifest_artifact_store_options<'a>(
    manifest: &'a BacktestingRunManifest,
    key: &'static str,
) -> Vec<(&'static str, &'a str)> {
    let mut values = Vec::new();
    if let Some(value) = manifest.artifact_store.storage_options.get(key) {
        values.push(("manifest.artifact_store.storage_options", value.as_str()));
    }
    if let Some(value) = manifest.artifact_store.rust_storage_options.get(key) {
        values.push((
            "manifest.artifact_store.rust_storage_options",
            value.as_str(),
        ));
    }
    values
}

/// Instrument specs for the run-spec's projected tables.
///
/// Existing single-table run-specs deserialize through the `Single` arm
/// unchanged (the run-spec hash is the SHA-256 of the raw TOML bytes, so their
/// hashes never move). A multi-instrument object keys specs by
/// `canonical_instrument_key` exactly as the canonical rows carry it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RunSpecInstrumentSpecs {
    // Boxed: `CatalogInstrumentSpec` is ~480 bytes while the keyed map is ~24,
    // so an unboxed `Single` would bloat every `RunSpecInstrumentSpecs` to the
    // larger size (clippy::large_enum_variant). The box keeps both variants
    // small; serde deserializes `Box<T>` transparently for the untagged form.
    Single(Box<CatalogInstrumentSpec>),
    Keyed(BTreeMap<String, CatalogInstrumentSpec>),
}

impl RunSpecInstrumentSpecs {
    /// The one spec of a single-instrument run-spec; fails loud for keyed maps.
    ///
    /// # Errors
    ///
    /// Returns an error when the run-spec carries a keyed spec map.
    pub fn single(&self) -> Result<&CatalogInstrumentSpec> {
        match self {
            Self::Single(spec) => Ok(&**spec),
            Self::Keyed(specs) => anyhow::bail!(
                "run-spec instrument_spec is keyed ({} entries); this path requires a single spec",
                specs.len()
            ),
        }
    }

    #[cfg(test)]
    pub(crate) fn single_mut(&mut self) -> Option<&mut CatalogInstrumentSpec> {
        match self {
            Self::Single(spec) => Some(&mut **spec),
            Self::Keyed(_) => None,
        }
    }
}

/// Instrument identities for the run-spec's source object, symmetric with
/// [`RunSpecInstrumentSpecs`]: `Single` binds one identity to every row and
/// `Keyed` maps the source's configured instrument-key values to identities
/// (feeding the bar/delta keyed-identity resolution at normalization).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RunSpecInstrumentIdentities {
    Single(CanonicalInstrumentIdentity),
    Keyed(BTreeMap<String, CanonicalInstrumentIdentity>),
}

impl RunSpecInstrumentIdentities {
    /// The one identity of a single-instrument run-spec; fails loud for keyed maps.
    ///
    /// # Errors
    ///
    /// Returns an error when the run-spec carries a keyed identity map.
    pub fn single(&self) -> Result<&CanonicalInstrumentIdentity> {
        match self {
            Self::Single(identity) => Ok(identity),
            Self::Keyed(identities) => anyhow::bail!(
                "run-spec identity is keyed ({} entries); this path requires a single identity",
                identities.len()
            ),
        }
    }

    fn to_bar_identities(&self) -> crate::canonical_bars::BarInstrumentIdentities {
        match self {
            Self::Single(identity) => {
                crate::canonical_bars::BarInstrumentIdentities::Single(identity.clone())
            }
            Self::Keyed(identities) => {
                crate::canonical_bars::BarInstrumentIdentities::Keyed(identities.clone())
            }
        }
    }

    fn to_delta_identities(&self) -> crate::canonical_order_book_deltas::DeltaInstrumentIdentities {
        match self {
            Self::Single(identity) => {
                crate::canonical_order_book_deltas::DeltaInstrumentIdentities::Single(
                    identity.clone(),
                )
            }
            Self::Keyed(identities) => {
                crate::canonical_order_book_deltas::DeltaInstrumentIdentities::Keyed(
                    identities.clone(),
                )
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn single_mut(&mut self) -> Option<&mut CanonicalInstrumentIdentity> {
        match self {
            Self::Single(identity) => Some(identity),
            Self::Keyed(_) => None,
        }
    }
}

/// Selector provenance hashes carried by an L2 replay run-spec, minted by the
/// upstream selection lane that produced the accepted object.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunSpecSelectorProvenance {
    pub event_count_ledger_hash: String,
    pub selected_asset_ids_hash: String,
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
    pub canonical_catalog_uri: Option<String>,
    pub nt_catalog_capability_plan: Option<NtCatalogCapabilityPlan>,
    pub nt_catalog_capability_proof_artifact: Option<NtCatalogCapabilityProofArtifact>,
    pub create_only_probe_transcript: Option<CreateOnlyProbeTranscript>,
    pub persisted_catalog_projection: Option<PersistedCatalogProjection>,
    pub persisted_catalog_objects: Vec<PersistedCatalogProjectionObject>,
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
        nt_catalog_manifest_uri: None,
        catalog_metadata_uri: portable_artifact_uri(&manifest.output_prefix, CATALOG_METADATA_FILE),
        result_contract_uri: portable_artifact_uri(&manifest.output_prefix, RESULT_CONTRACT_FILE),
    }
}

fn redact_operator_contract(output: &mut BacktestRunOutput, local_catalog_root: &Path) {
    stabilize_operator_contract_nt_result(&mut output.contract);
    let local_catalog_root = local_catalog_root.to_string_lossy();
    if !local_catalog_root.is_empty() {
        let portable_catalog_uri = output.contract.artifact_uris.nt_catalog_uri.clone();
        replace_contract_claim_limit_uri(
            &mut output.contract,
            local_catalog_root.as_ref(),
            &portable_catalog_uri,
        );
    }
}

fn stabilize_operator_contract_nt_result(contract: &mut BacktestResultContract) {
    contract.nt_result.machine_id = OPERATOR_ATTESTED_REDACTED.to_string();
    contract.nt_result.instance_id = OPERATOR_ATTESTED_REDACTED.to_string();
    contract.nt_result.elapsed_time_secs = OPERATOR_ATTESTED_ELAPSED_TIME_SECS;
}

fn replace_contract_claim_limit_uri(
    contract: &mut BacktestResultContract,
    from_uri: &str,
    to_uri: &str,
) {
    if from_uri.is_empty() || from_uri == to_uri {
        return;
    }
    for claim_limit in &mut contract.claim_limits {
        *claim_limit = claim_limit.replace(from_uri, to_uri);
    }
}

async fn persist_durable_contract_artifact(
    writer: &CreateOnlyArtifactWriter<'_>,
    store: &dyn ObjectStore,
    artifact_root: &ResolvedArtifactRoot,
    local_path: &Path,
    uri: &str,
    committed: bool,
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<()> {
    let path = artifact_root.object_path_for_uri(uri)?;
    work_budget.check_deadline(OperatorWorkBudgetStage::Publish)?;
    let payload = fs::read(local_path).with_context(|| {
        format!(
            "read durable contract artifact {} for {}",
            local_path.display(),
            uri
        )
    })?;
    if committed {
        let existing = read_object_store_object(store, &path)
            .await?
            .with_context(|| format!("committed durable artifact {uri} is missing"))?;
        ensure!(
            existing.as_ref() == payload.as_slice(),
            "committed durable artifact {uri} has different bytes"
        );
    } else {
        writer
            .put_create_idempotent(&path, payload)
            .await
            .with_context(|| format!("persist durable contract artifact {uri}"))?;
    }
    work_budget.check_deadline(OperatorWorkBudgetStage::Publish)?;
    Ok(())
}

async fn persist_durable_contract_artifacts(
    writer: &CreateOnlyArtifactWriter<'_>,
    store: &dyn ObjectStore,
    artifact_root: &ResolvedArtifactRoot,
    artifacts: &RunArtifacts,
    output_prefix: &str,
    committed: bool,
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<()> {
    let uris = &artifacts.output.contract.artifact_uris;
    persist_durable_contract_artifact(
        writer,
        store,
        artifact_root,
        &artifacts.proof_path,
        &uris.source_proof_uri,
        committed,
        work_budget,
    )
    .await?;
    persist_durable_contract_artifact(
        writer,
        store,
        artifact_root,
        &artifacts.canonical_artifact_path,
        &uris.canonical_table_uri,
        committed,
        work_budget,
    )
    .await?;
    persist_durable_contract_artifact(
        writer,
        store,
        artifact_root,
        &artifacts.catalog_metadata_path,
        &uris.catalog_metadata_uri,
        committed,
        work_budget,
    )
    .await?;
    persist_durable_contract_artifact(
        writer,
        store,
        artifact_root,
        &artifacts.contract_path,
        &uris.result_contract_uri,
        committed,
        work_budget,
    )
    .await?;
    persist_durable_contract_artifact(
        writer,
        store,
        artifact_root,
        &artifacts.run_manifest_path,
        &portable_artifact_uri(output_prefix, BACKTEST_RUN_MANIFEST_FILE),
        committed,
        work_budget,
    )
    .await?;
    persist_durable_contract_artifact(
        writer,
        store,
        artifact_root,
        &artifacts.conversion_manifest_path,
        &portable_artifact_uri(
            output_prefix,
            crate::conversion_boundary::CONVERSION_MANIFEST_FILE,
        ),
        committed,
        work_budget,
    )
    .await?;
    Ok(())
}

async fn read_object_store_object(
    store: &dyn ObjectStore,
    path: &ObjectPath,
) -> Result<Option<Bytes>> {
    let object = match store.get(path).await {
        Ok(object) => object,
        Err(ObjectStoreError::NotFound { .. }) => return Ok(None),
        Err(error) => return Err(error).with_context(|| format!("read object-store path {path}")),
    };
    object
        .bytes()
        .await
        .map(Some)
        .with_context(|| format!("read object-store bytes {path}"))
}

async fn commit_durable_checkpoint(
    store: &dyn ObjectStore,
    checkpoint_path: &ObjectPath,
    checkpoint_bytes: &[u8],
    _permit: OperatorWorkBudgetCommitPermit,
) -> Result<()> {
    match store
        .put_opts(
            checkpoint_path,
            Bytes::copy_from_slice(checkpoint_bytes).into(),
            PutMode::Create.into(),
        )
        .await
    {
        Ok(_) => Ok(()),
        Err(ObjectStoreError::AlreadyExists { .. })
        | Err(ObjectStoreError::Precondition { .. }) => {
            let existing = read_object_store_object(store, checkpoint_path)
                .await?
                .context("durable completion checkpoint disappeared after create conflict")?;
            ensure!(
                existing.as_ref() == checkpoint_bytes,
                "durable completion checkpoint won a create race with different bytes"
            );
            Ok(())
        }
        Err(error) => Err(error).context("create durable completion checkpoint"),
    }
}

fn verify_completed_result_contract(
    path: &Path,
    contract: &BacktestResultContract,
) -> Result<BacktestResultContract> {
    let existing = read_json_artifact::<BacktestResultContract>(path)?;
    let mut normalized = contract.clone();
    normalized.nt_result.machine_id = existing.nt_result.machine_id.clone();
    normalized.nt_result.instance_id = existing.nt_result.instance_id.clone();
    normalized.nt_result.elapsed_time_secs = existing.nt_result.elapsed_time_secs;
    ensure!(
        existing == normalized,
        "existing result contract {} differs from newly generated stable content",
        path.display()
    );
    Ok(existing)
}

fn validate_converter_config(converter: &ConverterConfig) -> Result<()> {
    ensure!(
        !converter.version.trim().is_empty(),
        "run-spec converter.version must not be empty"
    );
    let adapter = require_registered_source_adapter(&converter.identity, &converter.version)?;
    validate_raw_payload_config(&converter.raw_payload)?;
    ensure_container_matches_adapter_kind(adapter.kind, converter.raw_payload.container)?;
    Ok(())
}

/// Fail fast (before any artifact write) when the run-spec pairs a registered
/// adapter kind with a payload container it cannot consume: the decode boundary
/// produces one payload shape per container and each per-kind dispatcher
/// accepts exactly one shape.
fn ensure_container_matches_adapter_kind(
    kind: SourceAdapterKind,
    container: RawPayloadContainer,
) -> Result<()> {
    let admissible = match kind {
        SourceAdapterKind::CsvNativeTrades | SourceAdapterKind::CsvNativeBars => matches!(
            container,
            RawPayloadContainer::CsvGzip
                | RawPayloadContainer::CsvText
                | RawPayloadContainer::SingleCsvZip
        ),
        SourceAdapterKind::PagedJsonBars
        | SourceAdapterKind::JsonlMultiIntervalBars
        | SourceAdapterKind::JsonlSnapshotDeltas
        | SourceAdapterKind::SnapshotQuotes => matches!(
            container,
            RawPayloadContainer::JsonlText
                | RawPayloadContainer::JsonlGzip
                | RawPayloadContainer::SingleJsonlZip
        ),
        SourceAdapterKind::SeededL2Quotes => matches!(
            container,
            RawPayloadContainer::JsonlText
                | RawPayloadContainer::JsonlGzip
                | RawPayloadContainer::SingleJsonlZip
                | RawPayloadContainer::TarGzipJsonl
        ),
        SourceAdapterKind::TarJsonlSnapshotDeltas => {
            matches!(container, RawPayloadContainer::TarGzipJsonl)
        }
        SourceAdapterKind::ParquetEventStreamDeltas => {
            matches!(container, RawPayloadContainer::ParquetFile)
        }
        // The index-price raw normalizer (and thus its container shape) is not
        // yet built — data acquisition is tracked by bolt-v2 #836/#437. No
        // container is admissible, so a run-spec naming the index adapter fails
        // loud here at config validation; the canonical->NT projection path is
        // exercised directly by the synthetic round-trip tests, not via this raw
        // decode boundary.
        SourceAdapterKind::IndexPrices => false,
        // The mark-price raw normalizer (and thus its container shape) is not yet
        // built — data acquisition is tracked by bolt-v2 #836/#437. No container
        // is admissible, so a run-spec naming the mark adapter fails loud here
        // at config validation; the canonical->NT projection path is exercised
        // directly by the synthetic round-trip tests, not via this raw decode
        // boundary.
        SourceAdapterKind::MarkPrices => false,
        // The funding-rate raw normalizer (and thus its container shape) is not
        // yet built; data acquisition is tracked by bolt-v2 #836/#437. No
        // container is admissible, so a run-spec naming the funding adapter
        // fails loud here at config validation; the registered seam keeps
        // funding on the same operator path as index/mark while failing before
        // raw decode.
        SourceAdapterKind::FundingRates => false,
        #[cfg(test)]
        SourceAdapterKind::SyntheticOrderBookDeltas => false,
    };
    ensure!(
        admissible,
        "converter.raw_payload.container {container:?} is not admissible for adapter kind {kind:?}"
    );
    Ok(())
}

fn validate_converter_table_family(converter: &ConverterConfig, table_family: &str) -> Result<()> {
    require_registered_source_adapter_for_table_family(
        &converter.identity,
        &converter.version,
        table_family,
    )?;
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
        RawPayloadContainer::CsvGzip
        | RawPayloadContainer::CsvText
        | RawPayloadContainer::JsonlText
        | RawPayloadContainer::JsonlGzip
        | RawPayloadContainer::SingleJsonlZip
        | RawPayloadContainer::ParquetFile => {
            ensure!(
                config.zip_member.is_none(),
                "converter.raw_payload.zip_member is only valid for single_csv_zip"
            );
            ensure!(
                config.max_member_bytes.is_none(),
                "converter.raw_payload.max_member_bytes is only valid for tar_gzip_jsonl"
            );
            ensure!(
                config.member_suffix.is_none(),
                "converter.raw_payload.member_suffix is only valid for tar_gzip_jsonl"
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
            ensure!(
                config.max_member_bytes.is_none(),
                "converter.raw_payload.max_member_bytes is only valid for tar_gzip_jsonl"
            );
            ensure!(
                config.member_suffix.is_none(),
                "converter.raw_payload.member_suffix is only valid for tar_gzip_jsonl"
            );
        }
        RawPayloadContainer::TarGzipJsonl => {
            ensure!(
                config.zip_member.is_none(),
                "converter.raw_payload.zip_member is only valid for single_csv_zip"
            );
            ensure!(
                config
                    .member_suffix
                    .as_ref()
                    .is_some_and(|suffix| !suffix.trim().is_empty()),
                "converter.raw_payload.member_suffix is required for tar_gzip_jsonl"
            );
            ensure!(
                config.max_member_bytes.is_some_and(|bytes| bytes > 0),
                "converter.raw_payload.max_member_bytes is required and must be positive for tar_gzip_jsonl"
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

fn read_source_binding_registry(path: &Path) -> Result<SourceBindingRegistry> {
    read_source_binding_registry_from_path(path)
        .with_context(|| format!("read source-bindings registry {}", path.display()))
}

fn accepted_dataset_for_run_spec_hash_with_registry(
    spec: &RunSpec,
    object_sha256: &str,
    registry: &SourceBindingRegistry,
) -> Result<(SourceProofReport, AcceptedDataset)> {
    ensure!(
        spec.source_proof.is_accepted(),
        "source proof is not accepted: status {:?}",
        spec.source_proof.status
    );
    ensure!(
        spec.source_proof.accepted_by.as_deref() == Some(spec.accepted_by.as_str()),
        "source proof accepted_by does not match run-spec accepted_by"
    );
    ensure!(
        spec.source_proof.accepted_at.as_deref() == Some(spec.accepted_at_utc.as_str()),
        "source proof accepted_at does not match run-spec accepted_at_utc"
    );
    let accepted = select_accepted_dataset_with_registry(
        &spec.source_proof,
        &spec.accepted_object,
        object_sha256,
        registry,
    )
    .map_err(|error| anyhow::anyhow!("accepted-data ledger rejected object: {error}"))?;
    Ok((spec.source_proof.clone(), accepted))
}

fn local_run_manifest_for_output(
    spec: &RunSpec,
    output_dir: &Path,
) -> Result<BacktestingRunManifest> {
    let catalog_path = output_dir
        .join(CATALOG_DIR)
        .to_str()
        .context("catalog path is not valid UTF-8")?
        .to_string();
    let mut manifest = spec.manifest.clone();
    {
        let catalog_input = manifest.single_catalog_input_mut().map_err(|error| {
            anyhow::anyhow!("local catalog manifest requires one catalog input: {error}")
        })?;
        catalog_input.catalog_path = catalog_path;
        catalog_input.catalog_fs_protocol = CATALOG_FS_PROTOCOL_NONE.to_string();
        catalog_input.catalog_fs_storage_options.clear();
        catalog_input.catalog_fs_rust_storage_options.clear();
    }
    Ok(manifest)
}

fn validate_local_run_manifest(
    manifest: &BacktestingRunManifest,
    accepted: &AcceptedDataset,
) -> Result<()> {
    manifest
        .validate(accepted)
        .map_err(|error| anyhow::anyhow!("manifest validation failed: {error}"))
}

/// Validate every run-spec surface that does not require reading object bytes.
///
/// The caller may pass the accepted object hash from the run-spec for preflight
/// validation. [`run_from_run_spec`] still recomputes the hash from object bytes
/// before conversion or backtest execution.
pub fn validate_run_spec_manifest_for_object_hash(
    spec: &RunSpec,
    output_dir: &Path,
    object_sha256: &str,
) -> Result<()> {
    let registry = read_source_binding_registry(&spec.source_bindings_path)?;
    validate_run_spec_manifest_for_object_hash_with_registry(
        spec,
        output_dir,
        object_sha256,
        &registry,
    )
}

/// Validate every pre-payload run-spec gate against an already parsed exact
/// source-binding registry snapshot.
///
/// # Errors
///
/// Returns an error when converter, source-proof ledger, or manifest
/// validation fails.
pub fn validate_run_spec_manifest_for_object_hash_with_registry(
    spec: &RunSpec,
    output_dir: &Path,
    object_sha256: &str,
    registry: &SourceBindingRegistry,
) -> Result<()> {
    validate_converter_config(&spec.converter)?;
    let (_, accepted) =
        accepted_dataset_for_run_spec_hash_with_registry(spec, object_sha256, registry)?;
    validate_converter_table_family(&spec.converter, &accepted.table_family)?;
    if spec.manifest.catalog_inputs.len() == 1 {
        let manifest = local_run_manifest_for_output(spec, output_dir)?;
        validate_local_run_manifest(&manifest, &accepted)
    } else {
        // Multi-input manifests bind each input's catalog_path to its projected
        // per-table subroot only after normalization; preflight validates every
        // other gate-4 surface against the declared (placeholder-path) inputs.
        validate_local_run_manifest(&spec.manifest, &accepted)
    }
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
        "decoded text byte length {} exceeds converter.raw_payload.max_decoded_bytes {max_decoded_bytes}",
        bytes.len()
    );
    String::from_utf8(bytes).with_context(|| format!("decode {context_label} as UTF-8 text"))
}

/// One decoded accepted-object payload, after container decoding.
///
/// The container concern (decompress, walk zip/tar, passthrough) lives here at
/// the decode boundary; the per-kind dispatchers in `canonical_trades` consume
/// the matching shape. `Text` carries one bounded UTF-8 string (CSV or JSONL);
/// `TarMembers` carries the per-member-bounded JSONL members in archive order;
/// `ParquetBytes` carries the raw object bytes for columnar reads downstream.
enum DecodedPayload {
    Text(String),
    TarMembers(Vec<crate::tar_reader::TarMember>),
    ParquetBytes(Vec<u8>),
}

fn decode_object_payload(config: &RawPayloadConfig, object_bytes: &[u8]) -> Result<DecodedPayload> {
    validate_raw_payload_config(config)?;
    match config.container {
        RawPayloadContainer::CsvGzip => Ok(DecodedPayload::Text(read_limited_csv_text(
            flate2::read::GzDecoder::new(object_bytes),
            config.max_decoded_bytes,
            "gzip csv object",
        )?)),
        RawPayloadContainer::CsvText => Ok(DecodedPayload::Text(read_limited_csv_text(
            Cursor::new(object_bytes),
            config.max_decoded_bytes,
            "plain csv object",
        )?)),
        RawPayloadContainer::JsonlText => Ok(DecodedPayload::Text(read_limited_csv_text(
            Cursor::new(object_bytes),
            config.max_decoded_bytes,
            "plain jsonl object",
        )?)),
        RawPayloadContainer::JsonlGzip => Ok(DecodedPayload::Text(read_limited_csv_text(
            flate2::read::GzDecoder::new(object_bytes),
            config.max_decoded_bytes,
            "gzip jsonl object",
        )?)),
        RawPayloadContainer::SingleJsonlZip => {
            let mut member = crate::zip_reader::zip_member_reader(object_bytes)
                .context("open jsonl zip object")?;
            ensure!(
                member.declared_len() as u64 <= config.max_decoded_bytes,
                "ZIP JSONL member declared size {} exceeds converter.raw_payload.max_decoded_bytes {}",
                member.declared_len(),
                config.max_decoded_bytes
            );
            let text = read_limited_csv_text(
                &mut member,
                config.max_decoded_bytes,
                "single-member zip jsonl object",
            )?;
            member.verify().context("verify jsonl zip member")?;
            Ok(DecodedPayload::Text(text))
        }
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
            Ok(DecodedPayload::Text(read_limited_csv_text(
                member,
                config.max_decoded_bytes,
                &format!("zip member {member_name:?}"),
            )?))
        }
        RawPayloadContainer::TarGzipJsonl => {
            let member_suffix = config
                .member_suffix
                .as_deref()
                .context("converter.raw_payload.member_suffix is required for tar_gzip_jsonl")?;
            let max_member_bytes = config
                .max_member_bytes
                .context("converter.raw_payload.max_member_bytes is required for tar_gzip_jsonl")?;
            let mut members = Vec::new();
            for member in crate::tar_reader::gzip_tar_members(
                Cursor::new(object_bytes),
                member_suffix,
                max_member_bytes,
            ) {
                members.push(member.context("stream gzip tar jsonl member")?);
            }
            Ok(DecodedPayload::TarMembers(members))
        }
        RawPayloadContainer::ParquetFile => Ok(DecodedPayload::ParquetBytes(object_bytes.to_vec())),
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
    work_budget: &'a OperatorWorkBudgetGuard,
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
    let actual_metadata = actual_nt_market_data_metadata(&inputs.catalog_root)?;
    let canonical_rows = u64::try_from(canonical_table.rows.len())
        .context("canonical row count does not fit u64")?;
    let projected_row_groups =
        projected_nt_market_data_row_groups([u64::try_from(canonical_table.rows.len())
            .context("canonical row count does not fit u64")?])?;
    ensure!(
        actual_metadata.rows == canonical_rows,
        "completed actual projected Parquet metadata rows {} do not match canonical rows {canonical_rows}",
        actual_metadata.rows
    );
    ensure!(
        actual_metadata.row_groups == projected_row_groups,
        "completed actual projected row groups {} do not match expected {projected_row_groups}",
        actual_metadata.row_groups
    );
    inputs.work_budget.verify_actual_row_groups(
        actual_metadata.row_groups,
        OperatorWorkBudgetStage::CatalogProjection,
    )?;
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
    let manifest_catalog_input = inputs.manifest.single_catalog_input().map_err(|error| {
        anyhow::anyhow!("completed conversion check requires one catalog input: {error}")
    })?;
    ensure!(
        conversion_manifest.nt_instrument_id == manifest_catalog_input.nt_instrument_id,
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

    let crate::runner::NtBacktestNodeRun {
        result: nt_result,
        order_terminals,
        config_override_report,
        run_guard_report,
        ..
    } = run_nt_backtest_node_guarded(&inputs.manifest, inputs.work_budget)?;
    let expected = expected_iterations(
        &canonical_table.rows,
        inputs.manifest.start_time,
        inputs.manifest.end_time,
    )
    .context("compute expected engine iterations")?;
    if let Some(reason) = iterations_mismatch(nt_result.iterations, expected) {
        anyhow::bail!("backtest did not consume the accepted data: {reason}");
    }

    let mut claim_limits = inputs.accepted.result_contract_claim_limits();
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
        event_count_ledger_hash: None,
        selected_asset_ids_hash: None,
        strategy: &inputs.manifest.strategy,
        execution_model: &inputs.manifest.execution_model,
        venue_queue_position: inputs.manifest.venue.queue_position,
        catalog_data_types: inputs
            .manifest
            .catalog_inputs
            .iter()
            .map(|input| input.data_type.clone())
            .collect(),
        run_purpose: run_purpose_label(&inputs.manifest),
        market_structure_fixture: market_structure_label(&inputs.manifest),
        fidelity_class: canonical_table.fidelity_class,
        claim_limits,
        warnings: result_contract_warnings(&nt_result, canonical_table.fidelity_class),
        mechanical_blockers: Vec::new(),
        config_override_report: config_override_report.as_ref(),
        run_guard_report: run_guard_report.as_ref(),
        feed_labels: result_contract_feed_labels(&inputs.manifest),
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
        order_terminals,
        contract,
    };
    redact_operator_contract(&mut output, &inputs.catalog_root);
    output.contract = verify_completed_result_contract(&inputs.contract_path, &output.contract)?;

    atomic_write(
        &inputs.proof_path,
        serde_json::to_string_pretty(&inputs.accepted_source_proof)
            .context("serialize accepted source proof")?
            .as_bytes(),
    )
    .with_context(|| format!("write {}", inputs.proof_path.display()))?;
    atomic_write(
        &inputs.run_manifest_path,
        serde_json::to_string_pretty(&inputs.spec_manifest.to_artifact_manifest()?)
            .context("serialize resolved run manifest")?
            .as_bytes(),
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
        canonical_catalog_uri: None,
        nt_catalog_capability_plan: None,
        nt_catalog_capability_proof_artifact: None,
        create_only_probe_transcript: None,
        persisted_catalog_projection: None,
        persisted_catalog_objects: Vec::new(),
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
    run_from_run_spec_guarded(
        spec,
        object_bytes,
        output_dir,
        &OperatorWorkBudgetGuard::unbounded(),
    )
}

pub fn run_from_run_spec_guarded(
    spec: &RunSpec,
    object_bytes: &[u8],
    output_dir: &Path,
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<RunArtifacts> {
    let registry = read_source_binding_registry(&spec.source_bindings_path)?;
    run_from_run_spec_inner(spec, object_bytes, output_dir, true, &registry, work_budget)
}

fn run_from_run_spec_pending_guarded(
    spec: &RunSpec,
    object_bytes: &[u8],
    output_dir: &Path,
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<RunArtifacts> {
    let registry = read_source_binding_registry(&spec.source_bindings_path)?;
    run_from_run_spec_inner(
        spec,
        object_bytes,
        output_dir,
        false,
        &registry,
        work_budget,
    )
}

/// Run the vertical slice against the exact source-binding registry snapshot
/// already accepted by a caller's pre-payload control boundary.
///
/// # Errors
///
/// Returns the same errors as [`run_from_run_spec`], while guaranteeing the
/// run never reopens `RunSpec::source_bindings_path`.
pub fn run_from_run_spec_with_registry(
    spec: &RunSpec,
    object_bytes: &[u8],
    output_dir: &Path,
    registry: &SourceBindingRegistry,
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<RunArtifacts> {
    run_from_run_spec_inner(spec, object_bytes, output_dir, true, registry, work_budget)
}

fn run_from_run_spec_inner(
    spec: &RunSpec,
    object_bytes: &[u8],
    output_dir: &Path,
    reuse_completed_output: bool,
    source_binding_registry: &SourceBindingRegistry,
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<RunArtifacts> {
    work_budget.check_deadline(OperatorWorkBudgetStage::ObjectVerification)?;
    validate_converter_config(&spec.converter)?;
    let adapter =
        require_registered_source_adapter(&spec.converter.identity, &spec.converter.version)?;
    ensure!(
        adapter.kind == SourceAdapterKind::CsvNativeTrades,
        "run_from_run_spec is the single-table trade entry; adapter kind {:?} dispatches \
         through run_operator_from_run_spec",
        adapter.kind
    );
    ensure!(
        spec.selector_provenance.is_none(),
        "selector_provenance is only valid for L2 replay run-specs"
    );
    let identity = spec.identity.single()?;
    let instrument_spec = spec.instrument_spec.single()?;

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
    work_budget.check_deadline(OperatorWorkBudgetStage::ObjectVerification)?;
    ensure!(
        verified_sha256 == spec.accepted_object.sha256,
        "object SHA-256 {verified_sha256} does not match run-spec {}",
        spec.accepted_object.sha256
    );

    // Gate 1: accept the source proof and bind the object via the ledger.
    let (accepted_proof, accepted) = accepted_dataset_for_run_spec_hash_with_registry(
        spec,
        &verified_sha256,
        source_binding_registry,
    )?;
    validate_converter_table_family(&spec.converter, &accepted.table_family)?;

    let conversion_fingerprint = conversion_fingerprint_for(spec, &accepted)?;
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
    // Bind the manifest catalog input to the local projection root.
    let contract_manifest_hash = spec.manifest.manifest_hash();
    let manifest = local_run_manifest_for_output(spec, output_dir)?;
    validate_local_run_manifest(&manifest, &accepted)?;
    let artifact_uris = portable_artifact_uris(&manifest);

    work_budget.check_deadline(OperatorWorkBudgetStage::Decode)?;
    let DecodedPayload::Text(csv_text) =
        decode_object_payload(&spec.converter.raw_payload, object_bytes)?
    else {
        anyhow::bail!(
            "single-table trade entry requires a text payload container, got {:?}",
            spec.converter.raw_payload.container
        );
    };
    work_budget.check_deadline(OperatorWorkBudgetStage::Decode)?;

    if reuse_completed_output {
        match inspect_conversion_output(output_dir, &conversion_fingerprint)? {
            ConversionOutputState::Complete {
                manifest_hash,
                checkpoint_hash,
                catalog_hash,
            } => {
                normalize_registered_trade_converter(
                    &spec.converter,
                    &accepted,
                    identity,
                    &csv_text,
                    rfc3339_to_nanos(&spec.capture_time_utc)?,
                    &manifest.run_id,
                    work_budget,
                )
                .context("revalidate completed source rows against current work budget")?;
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
                    work_budget,
                });
            }
            ConversionOutputState::CleanNew
            | ConversionOutputState::ResumeFromCheckpoint { .. } => {}
        }
    }

    work_budget.check_deadline(OperatorWorkBudgetStage::CanonicalWrite)?;
    fs::create_dir_all(output_dir)
        .with_context(|| format!("create output dir {}", output_dir.display()))?;
    for stale_completed_artifact in [
        crate::conversion_boundary::CONVERSION_MANIFEST_FILE,
        crate::conversion_boundary::CATALOG_METADATA_FILE,
        PUBLISHED_CATALOG_PROOF_FILE,
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

    let mut output = run_backtest(BacktestRunInputs {
        accepted: &accepted,
        identity,
        instrument_spec,
        csv_text: &csv_text,
        capture_time_nanos: rfc3339_to_nanos(&spec.capture_time_utc)?,
        manifest: &manifest,
        contract_manifest_hash: &contract_manifest_hash,
        converter: &spec.converter,
        canonical_artifact_path: &canonical_path,
        catalog_root: &catalog_root,
        selector_provenance: None,
        created_at: &spec.created_at_utc,
        artifact_uris,
        work_budget,
    })?;
    redact_operator_contract(&mut output, &catalog_root);

    work_budget.check_deadline(OperatorWorkBudgetStage::Finalize)?;
    atomic_write(
        &proof_path,
        serde_json::to_string_pretty(&accepted_proof)
            .context("serialize accepted source proof")?
            .as_bytes(),
    )
    .with_context(|| format!("write {}", proof_path.display()))?;
    atomic_write(
        &contract_path,
        serde_json::to_string_pretty(&output.contract)
            .context("serialize result contract")?
            .as_bytes(),
    )
    .with_context(|| format!("write {}", contract_path.display()))?;
    atomic_write(
        &run_manifest_path,
        serde_json::to_string_pretty(&spec.manifest.to_artifact_manifest()?)
            .context("serialize resolved run manifest")?
            .as_bytes(),
    )
    .with_context(|| format!("write {}", run_manifest_path.display()))?;
    work_budget.check_deadline(OperatorWorkBudgetStage::Finalize)?;
    if reuse_completed_output {
        write_completed_conversion_artifacts_guarded(
            output_dir,
            &output.conversion_manifest,
            &output.conversion_checkpoint,
            &output.conversion_catalog_metadata,
            work_budget,
        )?;
    } else {
        write_pending_conversion_artifacts(
            output_dir,
            &output.conversion_manifest,
            &output.conversion_catalog_metadata,
        )?;
    }

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
        canonical_catalog_uri: None,
        nt_catalog_capability_plan: None,
        nt_catalog_capability_proof_artifact: None,
        create_only_probe_transcript: None,
        persisted_catalog_projection: None,
        persisted_catalog_objects: Vec::new(),
        output,
    })
}

/// Run the operator path and persist the projected NT catalog to the configured
/// artifact store through source-binding dispatch.
///
/// # Errors
///
/// Returns an error if the base run fails, artifact-store config is invalid, the
/// source binding cannot dispatch to one catalog root, or any create-only write
/// is rejected.
pub async fn run_from_run_spec_with_artifact_store<F>(
    spec: &RunSpec,
    gz_bytes: &[u8],
    output_dir: &Path,
    store: &dyn ObjectStore,
    build_capability_evidence: F,
) -> Result<RunArtifacts>
where
    F: FnOnce(
        &ResolvedArtifactRoot,
        &NtCatalogCapabilityPlan,
        CreateOnlyProbeTranscript,
    ) -> Result<NtCatalogCapabilityEvidence>,
{
    let work_budget = OperatorWorkBudgetGuard::unbounded();
    run_from_run_spec_with_artifact_store_guarded(
        spec,
        gz_bytes,
        output_dir,
        store,
        build_capability_evidence,
        &work_budget,
    )
    .await
}

/// Guarded durable-catalog operator path used by validated backfill callers.
pub async fn run_from_run_spec_with_artifact_store_guarded<F>(
    spec: &RunSpec,
    gz_bytes: &[u8],
    output_dir: &Path,
    store: &dyn ObjectStore,
    build_capability_evidence: F,
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<RunArtifacts>
where
    F: FnOnce(
        &ResolvedArtifactRoot,
        &NtCatalogCapabilityPlan,
        CreateOnlyProbeTranscript,
    ) -> Result<NtCatalogCapabilityEvidence>,
{
    let artifact_store = spec.required_artifact_store()?;
    spec.validate_artifact_store_publish_config(artifact_store)?;
    let catalog_dispatch = spec.required_catalog_dispatch()?;
    let create_only_probe_id = spec.required_create_only_probe_id()?;
    let nt_catalog_capability_proof = spec.required_nt_catalog_capability_proof()?;
    let base_spec = spec.clone();
    let base_gz_bytes = gz_bytes.to_vec();
    let base_output_dir = output_dir.to_path_buf();
    let source_binding_registry = read_source_binding_registry(&spec.source_bindings_path)?;
    let base_work_budget = work_budget.clone();
    let mut artifacts = tokio::task::spawn_blocking(move || {
        run_from_run_spec_inner(
            &base_spec,
            &base_gz_bytes,
            &base_output_dir,
            false,
            &source_binding_registry,
            &base_work_budget,
        )
    })
    .await
    .context("join base run for artifact-store path")??;
    work_budget.check_deadline(OperatorWorkBudgetStage::Publish)?;
    let artifact_root = artifact_store.resolve()?;
    let nt_catalog_capability_plan = nt_catalog_capability_proof.proof_plan(artifact_store)?;
    let completed_checkpoint_bytes =
        crate::reference_artifact::canonical_json_bytes(&artifacts.output.conversion_checkpoint)
            .context("serialize durable completion checkpoint")?;
    let completed_checkpoint_uri =
        portable_artifact_uri(&spec.manifest.output_prefix, CONVERSION_CHECKPOINT_FILE);
    let completed_checkpoint_path = artifact_root.object_path_for_uri(&completed_checkpoint_uri)?;
    work_budget.check_deadline(OperatorWorkBudgetStage::Publish)?;
    let remote_checkpoint = read_object_store_object(store, &completed_checkpoint_path).await?;
    work_budget.check_deadline(OperatorWorkBudgetStage::Publish)?;
    let remote_committed = if let Some(remote_checkpoint) = remote_checkpoint {
        ensure!(
            remote_checkpoint.as_ref() == completed_checkpoint_bytes.as_slice(),
            "durable completion checkpoint already exists with different bytes"
        );
        true
    } else {
        false
    };

    let writer = CreateOnlyArtifactWriter::new(store);
    // The run-prefix checkpoint commits only run-prefix objects. Global
    // capability/catalog roots retain their independent immutable protocols and
    // may create or verify content on an idempotent run-prefix replay.
    work_budget.check_deadline(OperatorWorkBudgetStage::Publish)?;
    let create_only_probe_transcript = writer
        .probe_create_only(&artifact_root, create_only_probe_id)
        .await?;
    work_budget.check_deadline(OperatorWorkBudgetStage::Publish)?;
    let nt_catalog_capability_evidence = build_capability_evidence(
        &artifact_root,
        &nt_catalog_capability_plan,
        create_only_probe_transcript.clone(),
    )?;
    work_budget.check_deadline(OperatorWorkBudgetStage::Publish)?;
    let nt_catalog_capability_proof_artifact = nt_catalog_capability_proof
        .persist_completed_proof_from_evidence(
            artifact_store,
            &writer,
            &nt_catalog_capability_evidence,
        )
        .await?;
    work_budget.check_deadline(OperatorWorkBudgetStage::Publish)?;
    let persisted = persist_catalog_projection_for_source_binding_guarded(
        store,
        &artifact_root,
        catalog_dispatch,
        &spec.source_proof.source_binding,
        spec.manifest.market_structure_fixture,
        &artifacts.catalog_root,
        work_budget,
    )
    .await?;
    work_budget.check_deadline(OperatorWorkBudgetStage::Publish)?;

    artifacts.output.conversion_catalog_metadata = artifacts
        .output
        .conversion_catalog_metadata
        .clone()
        .with_execution_catalog_access(persisted.catalog_root_uri.clone(), true);
    write_pending_conversion_artifacts(
        output_dir,
        &artifacts.output.conversion_manifest,
        &artifacts.output.conversion_catalog_metadata,
    )?;
    artifacts.output.contract.catalog_metadata_hash = artifacts
        .output
        .conversion_catalog_metadata
        .content_hash()
        .context("hash durable catalog metadata")?;
    let transient_catalog_uri = artifacts
        .output
        .contract
        .artifact_uris
        .nt_catalog_uri
        .clone();
    artifacts.output.contract.artifact_uris.nt_catalog_uri = persisted.catalog_root_uri.clone();
    replace_contract_claim_limit_uri(
        &mut artifacts.output.contract,
        &transient_catalog_uri,
        &persisted.catalog_root_uri,
    );
    artifacts
        .output
        .contract
        .artifact_uris
        .nt_catalog_manifest_uri = Some(persisted.manifest_uri.clone());
    artifacts
        .output
        .contract
        .validate()
        .map_err(|error| anyhow::anyhow!("durable result contract validation failed: {error}"))?;
    crate::reference_artifact::write_reference_artifact_with_len(
        &artifacts.contract_path,
        crate::result_contract::RESULT_CONTRACT_VERSION,
        &artifacts.output.contract,
        crate::reference_artifact::ReferenceArtifactRewrite::OverwriteAlways,
    )
    .with_context(|| format!("write {}", artifacts.contract_path.display()))?;
    persist_durable_contract_artifacts(
        &writer,
        store,
        &artifact_root,
        &artifacts,
        &spec.manifest.output_prefix,
        remote_committed,
        work_budget,
    )
    .await?;
    work_budget.check_deadline(OperatorWorkBudgetStage::Publish)?;
    write_completed_conversion_artifacts_guarded(
        output_dir,
        &artifacts.output.conversion_manifest,
        &artifacts.output.conversion_checkpoint,
        &artifacts.output.conversion_catalog_metadata,
        work_budget,
    )?;
    if !remote_committed {
        let permit = work_budget.authorize_commit(OperatorWorkBudgetStage::Publish)?;
        commit_durable_checkpoint(
            store,
            &completed_checkpoint_path,
            &completed_checkpoint_bytes,
            permit,
        )
        .await?;
    }
    artifacts.canonical_catalog_uri = Some(persisted.catalog_root_uri.clone());
    artifacts.nt_catalog_capability_plan = Some(nt_catalog_capability_plan);
    artifacts.nt_catalog_capability_proof_artifact = Some(nt_catalog_capability_proof_artifact);
    artifacts.create_only_probe_transcript = Some(create_only_probe_transcript);
    artifacts.persisted_catalog_objects = persisted.objects.clone();
    artifacts.persisted_catalog_projection = Some(persisted);
    if artifacts.catalog_root.exists() {
        fs::remove_dir_all(&artifacts.catalog_root).with_context(|| {
            format!(
                "remove transient local catalog root {}",
                artifacts.catalog_root.display()
            )
        })?;
    }
    Ok(artifacts)
}

/// Per-table catalog projection root directory under the artifact root.
pub const NT_CATALOGS_DIR: &str = "nt-catalogs";
/// Per-table canonical normalized Parquet artifact filename.
pub const CANONICAL_TABLE_FILE: &str = "canonical.parquet";
/// Subroot discriminant for table families without a per-table variant axis
/// (trades and order-book deltas); bars use `<step><aggregation>` lowercase.
pub const TABLE_DISCRIMINANT_DEFAULT: &str = "default";

/// One normalized canonical table produced by a registered adapter dispatch.
enum NormalizedTable {
    Trades(CanonicalTradesTable),
    Bars(CanonicalBarsTable),
    Deltas(CanonicalOrderBookDeltasTable),
    Quotes(CanonicalQuotesTable),
    Index(CanonicalIndexPricesTable),
    Mark(CanonicalMarkPricesTable),
    Funding(CanonicalFundingRatesTable),
}

impl NormalizedTable {
    fn table_family(&self) -> &'static str {
        match self {
            Self::Trades(_) => TRADE_TABLE_FAMILY,
            Self::Bars(_) => BAR_TABLE_FAMILY,
            Self::Deltas(_) => DELTAS_TABLE_FAMILY,
            Self::Quotes(_) => QUOTE_TABLE_FAMILY,
            Self::Index(_) => INDEX_PRICES_TABLE_FAMILY,
            Self::Mark(_) => MARK_PRICES_TABLE_FAMILY,
            Self::Funding(_) => FUNDING_RATES_TABLE_FAMILY,
        }
    }

    fn nt_data_type(&self) -> &'static str {
        match self {
            Self::Trades(_) => NT_DATA_TYPE_TRADE_TICK,
            Self::Bars(_) => NT_DATA_TYPE_BAR,
            Self::Deltas(_) => NT_DATA_TYPE_ORDER_BOOK_DELTA,
            Self::Quotes(_) => NT_DATA_TYPE_QUOTE_TICK,
            Self::Index(_) => NT_DATA_TYPE_INDEX_PRICE_UPDATE,
            Self::Mark(_) => NT_DATA_TYPE_MARK_PRICE_UPDATE,
            Self::Funding(_) => NT_DATA_TYPE_FUNDING_RATE_UPDATE,
        }
    }

    fn schema_version(&self) -> &str {
        match self {
            Self::Trades(table) => &table.schema_version,
            Self::Bars(table) => &table.schema_version,
            Self::Deltas(table) => &table.schema_version,
            Self::Quotes(table) => &table.schema_version,
            Self::Index(table) => &table.schema_version,
            Self::Mark(table) => &table.schema_version,
            Self::Funding(table) => &table.schema_version,
        }
    }

    fn fidelity_class(&self) -> SourceProofFidelityClass {
        match self {
            Self::Trades(table) => table.fidelity_class,
            Self::Bars(table) => table.fidelity_class,
            Self::Deltas(table) => table.fidelity_class,
            Self::Quotes(table) => table.fidelity_class,
            Self::Index(table) => table.fidelity_class,
            Self::Mark(table) => table.fidelity_class,
            Self::Funding(table) => table.fidelity_class,
        }
    }

    fn rows_len(&self) -> usize {
        match self {
            Self::Trades(table) => table.rows.len(),
            Self::Bars(table) => table.rows.len(),
            Self::Deltas(table) => table.rows.len(),
            Self::Quotes(table) => table.rows.len(),
            Self::Index(table) => table.rows.len(),
            Self::Mark(table) => table.rows.len(),
            Self::Funding(table) => table.rows.len(),
        }
    }

    fn nt_instrument_id(&self) -> Result<&str> {
        let id = match self {
            Self::Trades(table) => table
                .rows
                .first()
                .and_then(|row| row.nt_instrument_id.as_deref()),
            Self::Bars(table) => table
                .rows
                .first()
                .and_then(|row| row.nt_instrument_id.as_deref()),
            Self::Deltas(table) => table
                .rows
                .first()
                .and_then(|row| row.nt_instrument_id.as_deref()),
            Self::Quotes(table) => table
                .rows
                .first()
                .and_then(|row| row.nt_instrument_id.as_deref()),
            Self::Index(table) => table
                .rows
                .first()
                .and_then(|row| row.nt_instrument_id.as_deref()),
            Self::Mark(table) => table
                .rows
                .first()
                .and_then(|row| row.nt_instrument_id.as_deref()),
            Self::Funding(table) => table
                .rows
                .first()
                .and_then(|row| row.nt_instrument_id.as_deref()),
        };
        id.context("normalized table is missing rows[0].nt_instrument_id")
    }

    fn canonical_instrument_key(&self) -> Result<&str> {
        let key = match self {
            Self::Trades(table) => table
                .rows
                .first()
                .map(|row| row.canonical_instrument_key.as_str()),
            Self::Bars(table) => table
                .rows
                .first()
                .map(|row| row.canonical_instrument_key.as_str()),
            Self::Deltas(table) => table
                .rows
                .first()
                .map(|row| row.canonical_instrument_key.as_str()),
            Self::Quotes(table) => table
                .rows
                .first()
                .map(|row| row.canonical_instrument_key.as_str()),
            Self::Index(table) => table
                .rows
                .first()
                .map(|row| row.canonical_instrument_key.as_str()),
            Self::Mark(table) => table
                .rows
                .first()
                .map(|row| row.canonical_instrument_key.as_str()),
            Self::Funding(table) => table
                .rows
                .first()
                .map(|row| row.canonical_instrument_key.as_str()),
        };
        key.context("normalized table is missing rows[0].canonical_instrument_key")
    }

    /// Bar `<step><aggregation>` lowercase discriminant, `default` otherwise.
    fn discriminant(&self) -> String {
        match self {
            Self::Bars(table) => {
                format!("{}{}", table.bar_spec.step, table.bar_spec.aggregation).to_lowercase()
            }
            Self::Trades(_)
            | Self::Deltas(_)
            | Self::Quotes(_)
            | Self::Index(_)
            | Self::Mark(_)
            | Self::Funding(_) => TABLE_DISCRIMINANT_DEFAULT.to_string(),
        }
    }

    /// Min/max `ts_init` (engine nanos) across the table's rows, or `None` when
    /// empty. NautilusTrader replays and windows by `ts_init`
    /// (availability-or-capture), not the event clock, and the canonical rows are
    /// monotonic by `event_time` not `ts_init`, so the range is computed across
    /// all rows via the shared projection owner rather than read off first/last.
    ///
    /// # Errors
    ///
    /// Returns an error if a row's `ts_init` source clock is missing/non-positive.
    fn ts_init_range(&self) -> Result<Option<(u64, u64)>> {
        fn fold<R>(rows: &[R], ts_init: impl Fn(&R) -> Result<u64>) -> Result<Option<(u64, u64)>> {
            let mut range: Option<(u64, u64)> = None;
            for row in rows {
                let ts = ts_init(row)?;
                range = Some(match range {
                    Some((min, max)) => (min.min(ts), max.max(ts)),
                    None => (ts, ts),
                });
            }
            Ok(range)
        }
        match self {
            Self::Trades(table) => fold(&table.rows, |row| {
                Ok(ts_init_nanos(
                    row.availability_time,
                    row.capture_time,
                    &format!("trade {}", row.trade_id),
                )?
                .as_u64())
            }),
            Self::Bars(table) => fold(&table.rows, |row| {
                Ok(ts_init_nanos(
                    row.availability_time,
                    row.capture_time,
                    &format!("bar close_time {}", row.close_time),
                )?
                .as_u64())
            }),
            Self::Deltas(table) => fold(&table.rows, |row| {
                Ok(ts_init_nanos(
                    row.availability_time,
                    row.capture_time,
                    &format!("delta sequence {}", row.sequence),
                )?
                .as_u64())
            }),
            Self::Quotes(table) => fold(&table.rows, |row| {
                Ok(ts_init_nanos(
                    row.availability_time,
                    row.capture_time,
                    &format!("quote {}", row.event_time),
                )?
                .as_u64())
            }),
            Self::Index(table) => fold(&table.rows, |row| {
                Ok(ts_init_nanos(
                    row.availability_time,
                    row.capture_time,
                    &format!("index price {}", row.event_time),
                )?
                .as_u64())
            }),
            Self::Mark(table) => fold(&table.rows, |row| {
                Ok(ts_init_nanos(
                    row.availability_time,
                    row.capture_time,
                    &format!("mark price {}", row.event_time),
                )?
                .as_u64())
            }),
            Self::Funding(table) => fold(&table.rows, |row| {
                Ok(ts_init_nanos(
                    row.availability_time,
                    row.capture_time,
                    &format!("funding rate {}", row.event_time),
                )?
                .as_u64())
            }),
        }
    }

    /// Engine-delivery points inside the manifest's inclusive `[start, end]`
    /// window, mirroring [`expected_iterations`] for every projected family.
    /// NautilusTrader windows by `ts_init`, so the count is over each row's
    /// availability-or-capture clock (derived through the shared projection
    /// owner), never the event clock. Bounds are engine nanos.
    ///
    /// # Errors
    ///
    /// Returns an error if a row's `ts_init` source clock is missing/non-positive.
    fn windowed_count(&self, start: Option<u64>, end: Option<u64>) -> Result<usize> {
        fn count<R>(
            rows: &[R],
            start: Option<u64>,
            end: Option<u64>,
            ts_init: impl Fn(&R) -> Result<u64>,
        ) -> Result<usize> {
            let mut total = 0usize;
            for row in rows {
                let ts = ts_init(row)?;
                if start.is_none_or(|start| ts >= start) && end.is_none_or(|end| ts <= end) {
                    total += 1;
                }
            }
            Ok(total)
        }
        match self {
            Self::Trades(table) => count(&table.rows, start, end, |row| {
                Ok(ts_init_nanos(
                    row.availability_time,
                    row.capture_time,
                    &format!("trade {}", row.trade_id),
                )?
                .as_u64())
            }),
            Self::Bars(table) => count(&table.rows, start, end, |row| {
                Ok(ts_init_nanos(
                    row.availability_time,
                    row.capture_time,
                    &format!("bar close_time {}", row.close_time),
                )?
                .as_u64())
            }),
            Self::Deltas(table) => count(&table.rows, start, end, |row| {
                Ok(ts_init_nanos(
                    row.availability_time,
                    row.capture_time,
                    &format!("delta sequence {}", row.sequence),
                )?
                .as_u64())
            }),
            Self::Quotes(table) => count(&table.rows, start, end, |row| {
                Ok(ts_init_nanos(
                    row.availability_time,
                    row.capture_time,
                    &format!("quote {}", row.event_time),
                )?
                .as_u64())
            }),
            Self::Index(table) => count(&table.rows, start, end, |row| {
                Ok(ts_init_nanos(
                    row.availability_time,
                    row.capture_time,
                    &format!("index price {}", row.event_time),
                )?
                .as_u64())
            }),
            Self::Mark(table) => count(&table.rows, start, end, |row| {
                Ok(ts_init_nanos(
                    row.availability_time,
                    row.capture_time,
                    &format!("mark price {}", row.event_time),
                )?
                .as_u64())
            }),
            Self::Funding(table) => count(&table.rows, start, end, |row| {
                Ok(ts_init_nanos(
                    row.availability_time,
                    row.capture_time,
                    &format!("funding rate {}", row.event_time),
                )?
                .as_u64())
            }),
        }
    }
}

/// Replace NT-instrument-id path-hostile characters for catalog subroot use.
fn sanitized_catalog_component(value: &str) -> String {
    value.replace(['.', '/'], "_")
}

/// One normalized table bound to its per-table output locations.
struct PlannedTable {
    table: NormalizedTable,
    nt_instrument_id: String,
    bar_spec: Option<String>,
    subroot_relative: String,
    subroot: PathBuf,
    canonical_relative: String,
    canonical_path: PathBuf,
}

impl PlannedTable {
    fn record(&self, catalog_hash: String) -> ConversionTableRecord {
        ConversionTableRecord {
            table_family: self.table.table_family().to_string(),
            nt_instrument_id: self.nt_instrument_id.clone(),
            data_type: self.table.nt_data_type().to_string(),
            bar_spec: self.bar_spec.clone(),
            subroot_uri: self.subroot_relative.clone(),
            catalog_hash,
            rows: self.table.rows_len(),
        }
    }
}

/// Public per-table projection summary of a multi-table run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectedTableArtifacts {
    pub table_family: String,
    pub nt_instrument_id: String,
    pub data_type: String,
    pub bar_spec: Option<String>,
    pub subroot_relative: String,
    pub subroot: PathBuf,
    pub canonical_relative: String,
    pub canonical_path: PathBuf,
    pub rows: usize,
    pub catalog_hash: String,
}

/// Artifacts produced by a multi-table operator run.
pub struct MultiTableRunArtifacts {
    pub verified_sha256: String,
    pub accepted_source_proof: SourceProofReport,
    pub proof_path: PathBuf,
    pub contract_path: PathBuf,
    pub run_manifest_path: PathBuf,
    pub conversion_manifest_path: PathBuf,
    pub conversion_checkpoint_path: PathBuf,
    pub catalog_metadata_path: PathBuf,
    /// Present only when the conversion produced more than one table.
    pub conversion_tables_path: Option<PathBuf>,
    pub tables: Vec<ProjectedTableArtifacts>,
    pub conversion_checkpoint: ConversionCheckpoint,
    pub conversion_manifest: ConversionManifest,
    pub conversion_catalog_metadata: ConversionCatalogMetadata,
    pub conversion_checkpoint_hash: String,
    pub conversion_manifest_hash: String,
    pub nt_result: BacktestResult,
    pub contract: BacktestResultContract,
}

/// Operator run artifacts across both runner dispatches.
pub enum OperatorRunArtifacts {
    Trade(Box<RunArtifacts>),
    MultiTable(Box<MultiTableRunArtifacts>),
}

/// Run the operator for any registered adapter kind: the single-table trade
/// kind keeps its existing durable path, every other kind dispatches through
/// the multi-table flow (one object -> one manifest with N catalog inputs ->
/// ONE `BacktestNode` run).
///
/// # Errors
///
/// Returns an error if any gate of the dispatched flow fails.
pub fn run_operator_from_run_spec(
    spec: &RunSpec,
    object_bytes: &[u8],
    output_dir: &Path,
) -> Result<OperatorRunArtifacts> {
    run_operator_from_run_spec_guarded(
        spec,
        object_bytes,
        output_dir,
        &OperatorWorkBudgetGuard::unbounded(),
    )
}

pub fn run_operator_from_run_spec_guarded(
    spec: &RunSpec,
    object_bytes: &[u8],
    output_dir: &Path,
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<OperatorRunArtifacts> {
    let registry = read_source_binding_registry(&spec.source_bindings_path)?;
    run_operator_from_run_spec_with_registry(spec, object_bytes, output_dir, &registry, work_budget)
}

/// Dispatch any registered adapter against an already parsed source-binding
/// registry snapshot.
///
/// # Errors
///
/// Returns the same errors as [`run_operator_from_run_spec`] without reopening
/// `RunSpec::source_bindings_path` in the selected operator core.
pub fn run_operator_from_run_spec_with_registry(
    spec: &RunSpec,
    object_bytes: &[u8],
    output_dir: &Path,
    registry: &SourceBindingRegistry,
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<OperatorRunArtifacts> {
    let adapter =
        require_registered_source_adapter(&spec.converter.identity, &spec.converter.version)?;
    if adapter.kind == SourceAdapterKind::CsvNativeTrades {
        return Ok(OperatorRunArtifacts::Trade(Box::new(
            run_from_run_spec_with_registry(spec, object_bytes, output_dir, registry, work_budget)?,
        )));
    }
    Ok(OperatorRunArtifacts::MultiTable(Box::new(
        run_multi_table_from_run_spec_with_registry(
            spec,
            object_bytes,
            output_dir,
            registry,
            work_budget,
        )?,
    )))
}

fn conversion_fingerprint_for(
    spec: &RunSpec,
    accepted: &AcceptedDataset,
) -> Result<ConversionFingerprint> {
    Ok(ConversionFingerprint {
        source_proof_id: accepted.source_proof_id.clone(),
        source_proof_version: accepted.source_proof_version,
        accepted_object_sha256: accepted.accepted_object_sha256.clone(),
        converter_identity: spec.converter.identity.clone(),
        converter_version: spec.converter.version.clone(),
        converter_config_hash: spec
            .converter
            .content_hash()
            .context("hash converter config")?,
    })
}

/// Normalize the decoded payload through the registered adapter dispatch for
/// `kind`, producing every canonical table the object carries.
fn normalize_tables_for_kind(
    kind: SourceAdapterKind,
    spec: &RunSpec,
    accepted: &AcceptedDataset,
    payload: DecodedPayload,
    capture_time_nanos: i64,
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<Vec<NormalizedTable>> {
    let run_id = &spec.manifest.run_id;
    let tables: Vec<NormalizedTable> = match kind {
        SourceAdapterKind::CsvNativeBars => {
            let DecodedPayload::Text(text) = payload else {
                anyhow::bail!("CSV native-bars adapter requires a text payload container");
            };
            normalize_registered_bar_converter(
                &spec.converter,
                accepted,
                &spec.identity.to_bar_identities(),
                &text,
                capture_time_nanos,
                run_id,
                work_budget,
            )?
            .into_iter()
            .map(NormalizedTable::Bars)
            .collect()
        }
        SourceAdapterKind::PagedJsonBars => {
            let DecodedPayload::Text(text) = payload else {
                anyhow::bail!("paged-JSON bar adapter requires a text payload container");
            };
            normalize_registered_paged_json_bar_converter(
                &spec.converter,
                accepted,
                spec.identity.single()?,
                &text,
                capture_time_nanos,
                run_id,
                work_budget,
            )?
            .into_iter()
            .map(NormalizedTable::Bars)
            .collect()
        }
        SourceAdapterKind::JsonlMultiIntervalBars => {
            let DecodedPayload::Text(text) = payload else {
                anyhow::bail!("JSONL multi-interval bar adapter requires a text payload container");
            };
            normalize_registered_jsonl_multi_interval_bar_converter(
                &spec.converter,
                accepted,
                &spec.identity.to_bar_identities(),
                &text,
                capture_time_nanos,
                run_id,
                work_budget,
            )?
            .into_iter()
            .map(NormalizedTable::Bars)
            .collect()
        }
        SourceAdapterKind::JsonlSnapshotDeltas => {
            let DecodedPayload::Text(text) = payload else {
                anyhow::bail!("JSONL snapshot-delta adapter requires a text payload container");
            };
            normalize_registered_order_book_delta_converter(
                &spec.converter,
                accepted,
                &spec.identity.to_delta_identities(),
                &text,
                capture_time_nanos,
                run_id,
                work_budget,
            )?
            .into_iter()
            .map(NormalizedTable::Deltas)
            .collect()
        }
        SourceAdapterKind::TarJsonlSnapshotDeltas => {
            let DecodedPayload::TarMembers(members) = payload else {
                anyhow::bail!(
                    "tar JSONL snapshot-delta adapter requires the tar payload container"
                );
            };
            normalize_registered_tar_order_book_delta_converter(
                &spec.converter,
                accepted,
                &spec.identity.to_delta_identities(),
                members.into_iter().map(Ok),
                capture_time_nanos,
                run_id,
                work_budget,
            )?
            .into_iter()
            .map(NormalizedTable::Deltas)
            .collect()
        }
        SourceAdapterKind::ParquetEventStreamDeltas => {
            let DecodedPayload::ParquetBytes(bytes) = payload else {
                anyhow::bail!(
                    "Parquet event-stream delta adapter requires the parquet payload container"
                );
            };
            let (delta_tables, trade_tables) = normalize_registered_event_stream_delta_converter(
                &spec.converter,
                accepted,
                &spec.identity.to_delta_identities(),
                &bytes,
                capture_time_nanos,
                run_id,
                work_budget,
            )?;
            delta_tables
                .into_iter()
                .map(NormalizedTable::Deltas)
                .chain(trade_tables.into_iter().map(NormalizedTable::Trades))
                .collect::<Vec<_>>()
        }
        SourceAdapterKind::SnapshotQuotes => {
            let DecodedPayload::Text(text) = payload else {
                anyhow::bail!("snapshot-quotes adapter requires a text payload container");
            };
            // The snapshot-quotes wire normalizer is a registered seam; its
            // parsing path lands in a follow-up slice (it fails loud naming that
            // follow-up). The canonical quotes table + projection + read-back are
            // proven by the synthetic round-trip test in catalog_projection.
            normalize_registered_quote_converter(
                &spec.converter,
                accepted,
                spec.identity.single()?,
                &text,
                capture_time_nanos,
                run_id,
                work_budget,
            )?
            .into_iter()
            .map(NormalizedTable::Quotes)
            .collect()
        }
        SourceAdapterKind::SeededL2Quotes => match payload {
            DecodedPayload::Text(text) => normalize_registered_seeded_l2_quote_converter(
                &spec.converter,
                accepted,
                spec.identity.single()?,
                &text,
                capture_time_nanos,
                run_id,
                work_budget,
            )?
            .into_iter()
            .map(NormalizedTable::Quotes)
            .collect(),
            DecodedPayload::TarMembers(members) => {
                normalize_registered_tar_seeded_l2_quote_converter(
                    &spec.converter,
                    accepted,
                    spec.identity.single()?,
                    members,
                    capture_time_nanos,
                    run_id,
                    work_budget,
                )?
                .into_iter()
                .map(NormalizedTable::Quotes)
                .collect()
            }
            DecodedPayload::ParquetBytes(_) => {
                anyhow::bail!("seeded L2 quote adapter requires a text or tar payload container")
            }
        },
        SourceAdapterKind::IndexPrices => {
            let DecodedPayload::Text(text) = payload else {
                anyhow::bail!("index-price adapter requires a text payload container");
            };
            // The index-price wire normalizer (raw acquisition) is a registered
            // seam; its parsing path lands in a follow-up slice tracked by
            // bolt-v2 #836/#437, failing loud naming that follow-up. The
            // canonical index table + canonical->NT projection + read-back are
            // proven by the synthetic round-trip tests in catalog_projection.
            // Dispatching through the seam keeps NormalizedTable::Index on the
            // one normalization path (no parallel admittance logic).
            normalize_registered_index_converter(
                &spec.converter,
                accepted,
                spec.identity.single()?,
                &text,
                capture_time_nanos,
                run_id,
                work_budget,
            )?
            .into_iter()
            .map(NormalizedTable::Index)
            .collect()
        }
        SourceAdapterKind::MarkPrices => {
            let DecodedPayload::Text(text) = payload else {
                anyhow::bail!("mark-price adapter requires a text payload container");
            };
            // The mark-price wire normalizer (raw acquisition) is a registered
            // seam; its parsing path lands in a follow-up slice tracked by
            // bolt-v2 #836/#437, failing loud naming that follow-up. The
            // canonical mark table + canonical->NT projection + read-back are
            // proven by the synthetic round-trip tests in catalog_projection.
            // Dispatching through the seam keeps NormalizedTable::Mark on the
            // one normalization path (no parallel admittance logic).
            normalize_registered_mark_converter(
                &spec.converter,
                accepted,
                spec.identity.single()?,
                &text,
                capture_time_nanos,
                run_id,
                work_budget,
            )?
            .into_iter()
            .map(NormalizedTable::Mark)
            .collect()
        }
        SourceAdapterKind::FundingRates => {
            let DecodedPayload::Text(text) = payload else {
                anyhow::bail!("funding-rate adapter requires a text payload container");
            };
            // The funding-rate wire normalizer (raw acquisition) is a registered
            // seam; its parsing path lands in a follow-up slice tracked by
            // bolt-v2 #836/#437. The canonical funding table + canonical->NT
            // projection + read-back are proven by the synthetic round-trip
            // tests in catalog_projection. Dispatching through the seam keeps
            // NormalizedTable::Funding on the one normalization path.
            normalize_registered_funding_converter(
                &spec.converter,
                accepted,
                spec.identity.single()?,
                &text,
                capture_time_nanos,
                run_id,
                work_budget,
            )?
            .into_iter()
            .map(NormalizedTable::Funding)
            .collect()
        }
        SourceAdapterKind::CsvNativeTrades => {
            anyhow::bail!(
                "CSV native-trades adapter dispatches through the single-table trade entry"
            )
        }
        #[cfg(test)]
        SourceAdapterKind::SyntheticOrderBookDeltas => {
            anyhow::bail!("test fixture adapter has no operator dispatch")
        }
    };
    ensure!(
        !tables.is_empty(),
        "adapter dispatch produced no canonical tables"
    );
    Ok(tables)
}

/// Bind each normalized table to its per-table subroot and canonical artifact
/// locations under `output_dir`, rejecting duplicate table identities.
fn plan_projected_tables(
    output_dir: &Path,
    tables: Vec<NormalizedTable>,
) -> Result<Vec<PlannedTable>> {
    let mut planned = Vec::with_capacity(tables.len());
    let mut identities = std::collections::BTreeSet::new();
    for table in tables {
        let nt_instrument_id = table.nt_instrument_id()?.to_string();
        let discriminant = table.discriminant();
        let family = table.table_family();
        let sanitized_instrument = sanitized_catalog_component(&nt_instrument_id);
        ensure!(
            identities.insert((family, nt_instrument_id.clone(), discriminant.clone())),
            "duplicate projected table identity {family}/{nt_instrument_id}/{discriminant}"
        );
        let subroot_relative =
            format!("{NT_CATALOGS_DIR}/{family}/{sanitized_instrument}/{discriminant}");
        let canonical_relative =
            format!("{family}/{sanitized_instrument}/{discriminant}/{CANONICAL_TABLE_FILE}");
        let bar_spec = match &table {
            NormalizedTable::Bars(_) => Some(discriminant),
            NormalizedTable::Trades(_)
            | NormalizedTable::Deltas(_)
            | NormalizedTable::Quotes(_)
            | NormalizedTable::Index(_)
            | NormalizedTable::Mark(_)
            | NormalizedTable::Funding(_) => None,
        };
        planned.push(PlannedTable {
            subroot: output_dir.join(&subroot_relative),
            canonical_path: output_dir.join(&canonical_relative),
            table,
            nt_instrument_id,
            bar_spec,
            subroot_relative,
            canonical_relative,
        });
    }
    Ok(planned)
}

/// Resolve the run-spec instrument spec for one planned table.
fn resolve_instrument_spec<'a>(
    specs: &'a RunSpecInstrumentSpecs,
    planned: &PlannedTable,
    table_count: usize,
) -> Result<&'a CatalogInstrumentSpec> {
    match specs {
        RunSpecInstrumentSpecs::Single(spec) => {
            ensure!(
                table_count == 1,
                "run-spec instrument_spec is a single spec but the object produced \
                 {table_count} tables; key specs by canonical_instrument_key"
            );
            Ok(&**spec)
        }
        RunSpecInstrumentSpecs::Keyed(specs) => {
            let key = planned.table.canonical_instrument_key()?;
            specs.get(key).with_context(|| {
                format!(
                    "run-spec instrument_spec has no entry for canonical_instrument_key {key:?}"
                )
            })
        }
    }
}

/// Read the projected table back through NautilusTrader and prove count and
/// content equality against the canonical rows.
fn assert_planned_read_back(planned: &PlannedTable) -> Result<()> {
    match &planned.table {
        NormalizedTable::Trades(table) => {
            let ticks = read_back_trade_ticks(&planned.subroot, &planned.nt_instrument_id)
                .context("catalog read-back failed")?;
            ensure!(
                ticks.len() == table.rows.len(),
                "catalog read-back {} does not match projected {} trades",
                ticks.len(),
                table.rows.len()
            );
            assert_read_back_matches(&ticks, &table.rows, &planned.nt_instrument_id)
        }
        NormalizedTable::Bars(table) => {
            let bars = read_back_bars(&planned.subroot, &planned.nt_instrument_id)
                .context("catalog read-back failed")?;
            assert_bar_read_back_matches(&bars, table, &planned.nt_instrument_id)
        }
        NormalizedTable::Deltas(table) => {
            let deltas = read_back_order_book_deltas(&planned.subroot, &planned.nt_instrument_id)
                .context("catalog read-back failed")?;
            assert_delta_read_back_matches(&deltas, table, &planned.nt_instrument_id)
        }
        NormalizedTable::Quotes(table) => {
            let quotes = read_back_quotes(&planned.subroot, &planned.nt_instrument_id)
                .context("catalog read-back failed")?;
            assert_quote_read_back_matches(&quotes, table, &planned.nt_instrument_id)
        }
        NormalizedTable::Index(table) => {
            let prices = read_back_index(&planned.subroot, &planned.nt_instrument_id)
                .context("catalog read-back failed")?;
            assert_index_read_back_matches(&prices, table, &planned.nt_instrument_id)
        }
        NormalizedTable::Mark(table) => {
            let prices = read_back_mark(&planned.subroot, &planned.nt_instrument_id)
                .context("catalog read-back failed")?;
            assert_mark_read_back_matches(&prices, table, &planned.nt_instrument_id)
        }
        NormalizedTable::Funding(table) => {
            let rates = read_back_funding_rates(&planned.subroot, &planned.nt_instrument_id)
                .context("catalog read-back failed")?;
            assert_funding_read_back_matches(&rates, table, &planned.nt_instrument_id)
        }
    }
}

/// Bind every manifest catalog input to exactly one projected table and
/// rewrite its catalog path to the table's local subroot. Returns the bound
/// local manifest and, per input, the planned-table index it bound.
fn bind_catalog_inputs(
    spec_manifest: &BacktestingRunManifest,
    planned: &[PlannedTable],
) -> Result<(BacktestingRunManifest, Vec<usize>)> {
    let mut manifest = spec_manifest.clone();
    let mut used = vec![false; planned.len()];
    let mut bound_indices = Vec::with_capacity(manifest.catalog_inputs.len());
    for input in &mut manifest.catalog_inputs {
        let index = find_planned_table_for_input(input, planned, &used)?;
        used[index] = true;
        bound_indices.push(index);
        let catalog_path = planned[index]
            .subroot
            .to_str()
            .context("catalog subroot path is not valid UTF-8")?
            .to_string();
        input.catalog_path = catalog_path;
        input.catalog_fs_protocol = CATALOG_FS_PROTOCOL_NONE.to_string();
        input.catalog_fs_storage_options.clear();
        input.catalog_fs_rust_storage_options.clear();
    }
    if let Some(unused) = used.iter().position(|used| !used) {
        anyhow::bail!(
            "projected table {}/{} ({}) is not bound by any manifest catalog input",
            planned[unused].table.table_family(),
            planned[unused].nt_instrument_id,
            planned[unused].subroot_relative
        );
    }
    Ok((manifest, bound_indices))
}

fn find_planned_table_for_input(
    input: &ManifestCatalogInput,
    planned: &[PlannedTable],
    used: &[bool],
) -> Result<usize> {
    let candidates: Vec<usize> = planned
        .iter()
        .enumerate()
        .filter(|(index, table)| {
            !used[*index]
                && table.nt_instrument_id == input.nt_instrument_id
                && table.table.nt_data_type() == input.data_type
                && match (&input.bar_spec, &table.bar_spec) {
                    (Some(declared), Some(projected)) => declared == projected,
                    (None, _) => true,
                    (Some(_), None) => false,
                }
        })
        .map(|(index, _)| index)
        .collect();
    match candidates.as_slice() {
        [index] => Ok(*index),
        [] => anyhow::bail!(
            "manifest catalog input {}/{} (bar_spec {:?}) matches no projected table",
            input.nt_instrument_id,
            input.data_type,
            input.bar_spec
        ),
        _ => anyhow::bail!(
            "manifest catalog input {}/{} is ambiguous over {} projected tables; \
             declare bar_spec to disambiguate",
            input.nt_instrument_id,
            input.data_type,
            candidates.len()
        ),
    }
}

/// Reject a manifest time window that excludes every point of any projected
/// table (mirrors [`assert_time_window_overlaps_data`] per table). The overlap
/// is tested against each table's `ts_init` range — the clock NautilusTrader
/// windows by — not the event clock.
fn assert_tables_overlap_window(
    manifest: &BacktestingRunManifest,
    planned: &[PlannedTable],
) -> Result<()> {
    let start = window_bound_nanos("start_time", manifest.start_time)?;
    let end = window_bound_nanos("end_time", manifest.end_time)?;
    for table in planned {
        let Some((first, last)) = table.table.ts_init_range()? else {
            continue;
        };
        match time_window_excludes_all_data(start, end, first, last) {
            None => {}
            Some(bound) => anyhow::bail!(
                "manifest {bound} excludes all data of projected table {} (ts_init {first}..{last})",
                table.subroot_relative
            ),
        }
    }
    Ok(())
}

fn multi_artifact_uris(
    manifest: &BacktestingRunManifest,
    primary: &PlannedTable,
) -> ResultArtifactUris {
    ResultArtifactUris {
        source_proof_uri: portable_artifact_uri(
            &manifest.output_prefix,
            ACCEPTED_SOURCE_PROOF_FILE,
        ),
        canonical_table_uri: portable_artifact_uri(
            &manifest.output_prefix,
            &primary.canonical_relative,
        ),
        nt_catalog_uri: portable_artifact_uri(&manifest.output_prefix, &primary.subroot_relative),
        nt_catalog_manifest_uri: None,
        catalog_metadata_uri: portable_artifact_uri(&manifest.output_prefix, CATALOG_METADATA_FILE),
        result_contract_uri: portable_artifact_uri(&manifest.output_prefix, RESULT_CONTRACT_FILE),
    }
}

/// Redact host-locality from a multi-table contract: machine identity and
/// every local subroot path inside claim limits, replaced by the portable
/// published subroot URI.
fn redact_multi_operator_contract(
    contract: &mut BacktestResultContract,
    manifest: &BacktestingRunManifest,
    planned: &[PlannedTable],
) {
    stabilize_operator_contract_nt_result(contract);
    for table in planned {
        let local = table.subroot.to_string_lossy();
        if local.is_empty() {
            continue;
        }
        let portable = portable_artifact_uri(&manifest.output_prefix, &table.subroot_relative);
        for claim_limit in &mut contract.claim_limits {
            *claim_limit = claim_limit.replace(local.as_ref(), &portable);
        }
    }
}

/// Selector provenance rule for the multi-table flow: required exactly when
/// any projected table is `L2_REPLAY` (the result contract refuses an L2
/// contract without selection provenance), rejected otherwise.
fn multi_selector_provenance<'a>(
    spec: &'a RunSpec,
    planned: &[PlannedTable],
) -> Result<(Option<&'a str>, Option<&'a str>)> {
    let any_l2 = planned
        .iter()
        .any(|table| table.table.fidelity_class() == SourceProofFidelityClass::L2Replay);
    match (&spec.selector_provenance, any_l2) {
        (Some(provenance), true) => {
            ensure!(
                !provenance.event_count_ledger_hash.trim().is_empty(),
                "run-spec selector_provenance.event_count_ledger_hash must not be empty"
            );
            ensure!(
                !provenance.selected_asset_ids_hash.trim().is_empty(),
                "run-spec selector_provenance.selected_asset_ids_hash must not be empty"
            );
            Ok((
                Some(provenance.event_count_ledger_hash.as_str()),
                Some(provenance.selected_asset_ids_hash.as_str()),
            ))
        }
        (None, true) => anyhow::bail!(
            "L2 replay result contract requires run-spec selector_provenance \
             (event_count_ledger_hash + selected_asset_ids_hash)"
        ),
        (Some(_), false) => {
            anyhow::bail!("selector_provenance is only valid for L2 replay run-specs")
        }
        (None, false) => Ok((None, None)),
    }
}

/// Aggregate per-NT-data-type row totals across the projected tables.
fn rows_by_nt_data_type(planned: &[PlannedTable]) -> Result<BTreeMap<String, usize>> {
    let mut totals: BTreeMap<String, usize> = BTreeMap::new();
    for table in planned {
        let entry = totals
            .entry(table.table.nt_data_type().to_string())
            .or_insert(0);
        *entry = entry
            .checked_add(table.table.rows_len())
            .context("projected table row total overflow")?;
    }
    Ok(totals)
}

/// Run the multi-table operator flow for one accepted non-trade object.
///
/// One object -> N canonical tables -> N per-table catalog subroots + canonical
/// Parquet artifacts -> one bound N-input manifest -> ONE `BacktestNode` run ->
/// one conversion trio (plus the tables index when N > 1) and one result
/// contract bound to the primary catalog input.
///
/// # Errors
///
/// Returns an error if any gate fails: hash/ledger verification, adapter
/// dispatch, projection, read-back equality, manifest binding, the
/// `BacktestNode` iteration gate, or artifact verification on resume.
pub fn run_multi_table_from_run_spec(
    spec: &RunSpec,
    object_bytes: &[u8],
    output_dir: &Path,
) -> Result<MultiTableRunArtifacts> {
    let registry = read_source_binding_registry(&spec.source_bindings_path)?;
    run_multi_table_from_run_spec_with_registry(
        spec,
        object_bytes,
        output_dir,
        &registry,
        &OperatorWorkBudgetGuard::unbounded(),
    )
}

fn run_multi_table_from_run_spec_with_registry(
    spec: &RunSpec,
    object_bytes: &[u8],
    output_dir: &Path,
    registry: &SourceBindingRegistry,
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<MultiTableRunArtifacts> {
    work_budget.check_deadline(OperatorWorkBudgetStage::ObjectVerification)?;
    validate_converter_config(&spec.converter)?;
    let adapter =
        require_registered_source_adapter(&spec.converter.identity, &spec.converter.version)?;
    ensure!(
        adapter.kind != SourceAdapterKind::CsvNativeTrades,
        "CSV native-trades run-specs dispatch through the single-table trade entry"
    );

    let object_byte_len = object_bytes.len() as u64;
    ensure!(
        object_byte_len == spec.accepted_object.bytes,
        "object byte length {object_byte_len} does not match run-spec {}",
        spec.accepted_object.bytes
    );
    ensure_object_within_raw_payload_limit(&spec.converter.raw_payload, object_byte_len)?;

    let mut hasher = Sha256::new();
    hasher.update(object_bytes);
    let verified_sha256 = hex::encode(hasher.finalize());
    work_budget.check_deadline(OperatorWorkBudgetStage::ObjectVerification)?;
    ensure!(
        verified_sha256 == spec.accepted_object.sha256,
        "object SHA-256 {verified_sha256} does not match run-spec {}",
        spec.accepted_object.sha256
    );

    // Gate 1: accept the source proof and bind the object via the ledger.
    let (accepted_proof, accepted) =
        accepted_dataset_for_run_spec_hash_with_registry(spec, &verified_sha256, registry)?;
    validate_converter_table_family(&spec.converter, &accepted.table_family)?;
    // Gate 4 preflight on the declared (placeholder-path) inputs, before any
    // artifact is produced.
    validate_local_run_manifest(&spec.manifest, &accepted)?;

    let conversion_fingerprint = conversion_fingerprint_for(spec, &accepted)?;
    let contract_manifest_hash = spec.manifest.manifest_hash();
    let capture_time_nanos = rfc3339_to_nanos(&spec.capture_time_utc)?;

    let proof_path = output_dir.join(ACCEPTED_SOURCE_PROOF_FILE);
    let contract_path = output_dir.join(RESULT_CONTRACT_FILE);
    let run_manifest_path = output_dir.join(BACKTEST_RUN_MANIFEST_FILE);
    let conversion_manifest_path =
        output_dir.join(crate::conversion_boundary::CONVERSION_MANIFEST_FILE);
    let conversion_checkpoint_path =
        output_dir.join(crate::conversion_boundary::CONVERSION_CHECKPOINT_FILE);
    let catalog_metadata_path = output_dir.join(CATALOG_METADATA_FILE);

    let completed = match inspect_conversion_output(output_dir, &conversion_fingerprint)? {
        ConversionOutputState::Complete {
            manifest_hash,
            checkpoint_hash,
            catalog_hash,
        } => Some((manifest_hash, checkpoint_hash, catalog_hash)),
        ConversionOutputState::CleanNew | ConversionOutputState::ResumeFromCheckpoint { .. } => {
            None
        }
    };

    // Decode and normalize on both paths: the completed path re-derives the
    // canonical tables in memory to re-prove read-back equality and the
    // engine-iteration expectation without re-projecting verified subroots.
    work_budget.check_deadline(OperatorWorkBudgetStage::Decode)?;
    let payload = decode_object_payload(&spec.converter.raw_payload, object_bytes)?;
    work_budget.check_deadline(OperatorWorkBudgetStage::Decode)?;
    let tables = normalize_tables_for_kind(
        adapter.kind,
        spec,
        &accepted,
        payload,
        capture_time_nanos,
        work_budget,
    )?;
    let table_count = tables.len();
    let planned = plan_projected_tables(output_dir, tables)?;
    let projected_row_groups = projected_nt_market_data_row_groups(
        planned
            .iter()
            .map(|table| u64::try_from(table.table.rows_len()))
            .collect::<std::result::Result<Vec<_>, _>>()
            .context("projected canonical row count does not fit u64")?,
    )?;
    work_budget.check_projected_row_groups(
        projected_row_groups,
        OperatorWorkBudgetStage::CatalogProjection,
    )?;

    if let Some((manifest_hash, checkpoint_hash, primary_catalog_hash)) = completed {
        return run_multi_from_completed_output(MultiCompletedInputs {
            spec,
            accepted: &accepted,
            accepted_proof,
            verified_sha256,
            planned,
            conversion_manifest_hash: manifest_hash,
            conversion_checkpoint_hash: checkpoint_hash,
            primary_catalog_hash,
            contract_manifest_hash,
            output_dir,
            proof_path,
            contract_path,
            run_manifest_path,
            conversion_manifest_path,
            conversion_checkpoint_path,
            catalog_metadata_path,
            work_budget,
            projected_row_groups,
        });
    }

    work_budget.check_deadline(OperatorWorkBudgetStage::CanonicalWrite)?;
    fs::create_dir_all(output_dir)
        .with_context(|| format!("create output dir {}", output_dir.display()))?;
    for stale_completed_artifact in [
        crate::conversion_boundary::CONVERSION_MANIFEST_FILE,
        CATALOG_METADATA_FILE,
        CONVERSION_TABLES_FILE,
    ] {
        let path = output_dir.join(stale_completed_artifact);
        if path.exists() {
            fs::remove_file(&path).with_context(|| format!("remove {}", path.display()))?;
        }
    }
    write_conversion_checkpoint(
        output_dir,
        &ConversionCheckpoint::started(conversion_fingerprint.clone(), spec.created_at_utc.clone()),
    )?;
    // Start every projection from a clean tree (same governance as the trade
    // path's catalog-root clean): stale subroots or canonical artifacts must
    // never be silently re-stamped under a new source proof.
    for stale_tree in [
        NT_CATALOGS_DIR,
        TRADE_TABLE_FAMILY,
        BAR_TABLE_FAMILY,
        DELTAS_TABLE_FAMILY,
        QUOTE_TABLE_FAMILY,
        INDEX_PRICES_TABLE_FAMILY,
        MARK_PRICES_TABLE_FAMILY,
        FUNDING_RATES_TABLE_FAMILY,
    ] {
        let path = output_dir.join(stale_tree);
        if path.exists() {
            fs::remove_dir_all(&path).with_context(|| format!("clean {}", path.display()))?;
        }
    }

    // Gates 2+3 per table: projection, read-back, equality, canonical artifact.
    let mut catalog_hashes = Vec::with_capacity(planned.len());
    for table in &planned {
        work_budget.check_deadline(OperatorWorkBudgetStage::CatalogProjection)?;
        let instrument_spec = resolve_instrument_spec(&spec.instrument_spec, table, table_count)?;
        let projection = match &table.table {
            NormalizedTable::Trades(canonical) => {
                project_canonical_trades_to_catalog(canonical, instrument_spec, &table.subroot)
            }
            NormalizedTable::Bars(canonical) => {
                project_canonical_bars_to_catalog(canonical, instrument_spec, &table.subroot)
            }
            NormalizedTable::Deltas(canonical) => project_canonical_order_book_deltas_to_catalog(
                canonical,
                instrument_spec,
                &table.subroot,
            ),
            NormalizedTable::Quotes(canonical) => {
                project_canonical_quotes_to_catalog(canonical, instrument_spec, &table.subroot)
            }
            NormalizedTable::Index(canonical) => {
                project_canonical_index_to_catalog(canonical, instrument_spec, &table.subroot)
            }
            NormalizedTable::Mark(canonical) => {
                project_canonical_mark_to_catalog(canonical, instrument_spec, &table.subroot)
            }
            NormalizedTable::Funding(canonical) => project_canonical_funding_rates_to_catalog(
                canonical,
                instrument_spec,
                &table.subroot,
            ),
        }
        .with_context(|| format!("catalog projection failed for {}", table.subroot_relative))?;
        ensure!(
            projection.nt_instrument_id == table.nt_instrument_id,
            "projected instrument {:?} does not match canonical rows {:?}",
            projection.nt_instrument_id,
            table.nt_instrument_id
        );
        ensure!(
            projection.trade_count == table.table.rows_len(),
            "projection wrote {} data points for {} canonical rows",
            projection.trade_count,
            table.table.rows_len()
        );
        assert_planned_read_back(table)?;
        let parent = table
            .canonical_path
            .parent()
            .context("canonical artifact path has no parent")?;
        fs::create_dir_all(parent)
            .with_context(|| format!("create canonical artifact dir {}", parent.display()))?;
        work_budget.check_deadline(OperatorWorkBudgetStage::CanonicalWrite)?;
        match &table.table {
            NormalizedTable::Trades(canonical) => canonical.write_parquet(&table.canonical_path),
            NormalizedTable::Bars(canonical) => canonical.write_parquet(&table.canonical_path),
            NormalizedTable::Deltas(canonical) => canonical.write_parquet(&table.canonical_path),
            NormalizedTable::Quotes(canonical) => canonical.write_parquet(&table.canonical_path),
            NormalizedTable::Index(canonical) => canonical.write_parquet(&table.canonical_path),
            NormalizedTable::Mark(canonical) => canonical.write_parquet(&table.canonical_path),
            NormalizedTable::Funding(canonical) => canonical.write_parquet(&table.canonical_path),
        }
        .with_context(|| {
            format!(
                "write canonical artifact {}",
                table.canonical_path.display()
            )
        })?;
        catalog_hashes.push(projection.catalog_hash);
    }

    let (actual_rows, actual_row_groups) =
        planned
            .iter()
            .try_fold((0_u64, 0_u64), |(rows, row_groups), table| {
                let metadata = actual_nt_market_data_metadata(&table.subroot)?;
                Ok((
                    rows.checked_add(metadata.rows)
                        .context("actual projected row total overflow")?,
                    row_groups
                        .checked_add(metadata.row_groups)
                        .context("actual projected row-group total overflow")?,
                ))
            })?;
    let expected_rows = planned.iter().try_fold(0_u64, |rows, table| {
        rows.checked_add(
            u64::try_from(table.table.rows_len())
                .context("canonical row count does not fit u64")?,
        )
        .context("canonical row total overflow")
    })?;
    ensure!(
        actual_rows == expected_rows,
        "actual projected Parquet metadata rows {actual_rows} do not match canonical rows {expected_rows}"
    );
    ensure!(
        actual_row_groups == projected_row_groups,
        "actual projected row groups {actual_row_groups} do not match pre-write projection {projected_row_groups}"
    );
    work_budget.verify_actual_row_groups(
        actual_row_groups,
        OperatorWorkBudgetStage::CatalogProjection,
    )?;

    // Bind every manifest input to its projected table; gate 4 on the bound
    // manifest; per-table window overlap.
    let (local_manifest, bound_indices) = bind_catalog_inputs(&spec.manifest, &planned)?;
    validate_local_run_manifest(&local_manifest, &accepted)?;
    assert_tables_overlap_window(&local_manifest, &planned)?;
    let primary_index = *bound_indices
        .first()
        .context("manifest must declare at least one catalog input")?;
    let primary = &planned[primary_index];
    let primary_catalog_hash = catalog_hashes[primary_index].clone();
    let artifact_uris = multi_artifact_uris(&spec.manifest, primary);
    let (event_count_ledger_hash, selected_asset_ids_hash) =
        multi_selector_provenance(spec, &planned)?;

    // Gate 5: ONE BacktestNode run over the N-input manifest.
    let nt_run = run_nt_backtest_node_guarded(&local_manifest, work_budget)?;
    let nt_result = nt_run.result;
    let config_override_report = nt_run.config_override_report;
    let run_guard_report = nt_run.run_guard_report;
    let window_start = window_bound_nanos("start_time", local_manifest.start_time)?;
    let window_end = window_bound_nanos("end_time", local_manifest.end_time)?;
    let mut expected = 0usize;
    for table in &planned {
        expected += table
            .table
            .windowed_count(window_start, window_end)
            .context("compute expected engine iterations for projected table")?;
    }
    if let Some(reason) = iterations_mismatch(nt_result.iterations, expected) {
        anyhow::bail!("backtest did not consume the accepted data: {reason}");
    }

    // Conversion trio (aggregate) + tables index.
    let totals = rows_by_nt_data_type(&planned)?;
    let primary_data_type_rows = *totals
        .get(primary.table.nt_data_type())
        .context("primary data type missing from aggregate row totals")?;
    let conversion_checkpoint = ConversionCheckpoint::completed(
        conversion_fingerprint.clone(),
        primary_data_type_rows,
        primary_catalog_hash.clone(),
        spec.created_at_utc.clone(),
    );
    let conversion_checkpoint_hash = conversion_checkpoint
        .content_hash()
        .context("hash conversion checkpoint")?;
    let conversion_manifest = ConversionManifest::completed(
        conversion_fingerprint,
        primary.table.schema_version().to_string(),
        primary.table.nt_data_type().to_string(),
        primary.nt_instrument_id.clone(),
        primary_data_type_rows,
        artifact_uris.nt_catalog_uri.clone(),
        primary_catalog_hash.clone(),
        conversion_checkpoint_hash.clone(),
        spec.created_at_utc.clone(),
    )
    .with_catalog_rows_by_nt_data_type(totals);
    let conversion_manifest_hash = conversion_manifest
        .content_hash()
        .context("hash conversion manifest")?;
    // Keep the deterministic defaults from `from_manifest` (portable
    // output_catalog_uri; direct access = false). The transient local subroot
    // path must never enter the byte-deterministic catalog-metadata.json; the
    // portable execution URI is recorded later by the published-catalog proof.
    let conversion_catalog_metadata = ConversionCatalogMetadata::from_manifest(
        &conversion_manifest,
        conversion_manifest_hash.clone(),
        conversion_checkpoint_hash.clone(),
    );
    let conversion_catalog_metadata_hash = conversion_catalog_metadata
        .content_hash()
        .context("hash catalog metadata")?;

    // Gate 6: objective result contract bound to the primary catalog input.
    let mut claim_limits = accepted.result_contract_claim_limits();
    claim_limits.extend(nt_extension_surface_claim_limits(&local_manifest)?);
    let primary_fidelity = primary.table.fidelity_class();
    let mut contract = build_result_contract(ResultContractInputs {
        run_id: &local_manifest.run_id,
        source_proof_id: &accepted.source_proof_id,
        source_proof_version: accepted.source_proof_version,
        manifest_hash: &contract_manifest_hash,
        acceptance_mode: accepted.acceptance_mode,
        accepted_by: &accepted.accepted_by,
        accepted_at: &accepted.accepted_at,
        accepted_object_sha256: &accepted.accepted_object_sha256,
        converter_identity: &conversion_manifest.fingerprint.converter_identity,
        converter_version: &conversion_manifest.fingerprint.converter_version,
        converter_config_hash: &conversion_manifest.fingerprint.converter_config_hash,
        conversion_manifest_hash: &conversion_manifest_hash,
        conversion_checkpoint_hash: &conversion_checkpoint_hash,
        catalog_hash: &primary_catalog_hash,
        catalog_metadata_hash: &conversion_catalog_metadata_hash,
        event_count_ledger_hash,
        selected_asset_ids_hash,
        strategy: &local_manifest.strategy,
        execution_model: &local_manifest.execution_model,
        venue_queue_position: local_manifest.venue.queue_position,
        catalog_data_types: local_manifest
            .catalog_inputs
            .iter()
            .map(|input| input.data_type.clone())
            .collect(),
        run_purpose: run_purpose_label(&local_manifest),
        market_structure_fixture: market_structure_label(&local_manifest),
        fidelity_class: primary_fidelity,
        claim_limits,
        warnings: result_contract_warnings(&nt_result, primary_fidelity),
        mechanical_blockers: Vec::new(),
        config_override_report: config_override_report.as_ref(),
        run_guard_report: run_guard_report.as_ref(),
        feed_labels: result_contract_feed_labels(&local_manifest),
        nt_result: &nt_result,
        artifact_uris,
        created_at: &spec.created_at_utc,
    })
    .map_err(|error| anyhow::anyhow!("result contract construction failed: {error}"))?;
    redact_multi_operator_contract(&mut contract, &spec.manifest, &planned);

    work_budget.check_deadline(OperatorWorkBudgetStage::Finalize)?;
    atomic_write(
        &proof_path,
        serde_json::to_string_pretty(&accepted_proof)
            .context("serialize accepted source proof")?
            .as_bytes(),
    )
    .with_context(|| format!("write {}", proof_path.display()))?;
    atomic_write(
        &contract_path,
        serde_json::to_string_pretty(&contract)
            .context("serialize result contract")?
            .as_bytes(),
    )
    .with_context(|| format!("write {}", contract_path.display()))?;
    atomic_write(
        &run_manifest_path,
        serde_json::to_string_pretty(&spec.manifest.to_artifact_manifest()?)
            .context("serialize resolved run manifest")?
            .as_bytes(),
    )
    .with_context(|| format!("write {}", run_manifest_path.display()))?;
    let conversion_tables_path = if planned.len() > 1 {
        let records: Vec<ConversionTableRecord> = planned
            .iter()
            .zip(catalog_hashes.iter())
            .map(|(table, hash)| table.record(hash.clone()))
            .collect();
        Some(write_conversion_tables_index(output_dir, &records)?)
    } else {
        None
    };
    write_completed_conversion_artifacts_guarded(
        output_dir,
        &conversion_manifest,
        &conversion_checkpoint,
        &conversion_catalog_metadata,
        work_budget,
    )?;

    let tables = planned
        .iter()
        .zip(catalog_hashes.iter())
        .map(|(table, hash)| ProjectedTableArtifacts {
            table_family: table.table.table_family().to_string(),
            nt_instrument_id: table.nt_instrument_id.clone(),
            data_type: table.table.nt_data_type().to_string(),
            bar_spec: table.bar_spec.clone(),
            subroot_relative: table.subroot_relative.clone(),
            subroot: table.subroot.clone(),
            canonical_relative: table.canonical_relative.clone(),
            canonical_path: table.canonical_path.clone(),
            rows: table.table.rows_len(),
            catalog_hash: hash.clone(),
        })
        .collect();

    Ok(MultiTableRunArtifacts {
        verified_sha256,
        accepted_source_proof: accepted_proof,
        proof_path,
        contract_path,
        run_manifest_path,
        conversion_manifest_path,
        conversion_checkpoint_path,
        catalog_metadata_path,
        conversion_tables_path,
        tables,
        conversion_checkpoint,
        conversion_manifest,
        conversion_catalog_metadata,
        conversion_checkpoint_hash,
        conversion_manifest_hash,
        nt_result,
        contract,
    })
}

struct MultiCompletedInputs<'a> {
    spec: &'a RunSpec,
    accepted: &'a AcceptedDataset,
    accepted_proof: SourceProofReport,
    verified_sha256: String,
    planned: Vec<PlannedTable>,
    conversion_manifest_hash: String,
    conversion_checkpoint_hash: String,
    primary_catalog_hash: String,
    contract_manifest_hash: String,
    output_dir: &'a Path,
    proof_path: PathBuf,
    contract_path: PathBuf,
    run_manifest_path: PathBuf,
    conversion_manifest_path: PathBuf,
    conversion_checkpoint_path: PathBuf,
    catalog_metadata_path: PathBuf,
    work_budget: &'a OperatorWorkBudgetGuard,
    projected_row_groups: u64,
}

/// Reuse a completed multi-table output: re-prove every subroot hash and
/// read-back equality from the re-normalized tables, verify the conversion
/// trio and (for N > 1) the tables index, re-run the `BacktestNode` gate, and
/// require the regenerated result contract to be byte-stable.
fn run_multi_from_completed_output(
    inputs: MultiCompletedInputs<'_>,
) -> Result<MultiTableRunArtifacts> {
    let spec = inputs.spec;
    let accepted = inputs.accepted;
    let planned = inputs.planned;

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
    let conversion_catalog_metadata_hash = conversion_catalog_metadata
        .content_hash()
        .context("hash completed catalog metadata")?;

    // Recompute every projected subroot hash and prove read-back equality
    // against the re-normalized tables; bind the index records exactly when
    // the conversion produced more than one table.
    let mut catalog_hashes = Vec::with_capacity(planned.len());
    for table in &planned {
        let actual_hash = logical_catalog_hash(&table.subroot)
            .with_context(|| format!("verify catalog hash {}", table.subroot.display()))?;
        assert_planned_read_back(table)?;
        ensure!(
            table.canonical_path.is_file(),
            "completed conversion is missing canonical artifact {}",
            table.canonical_path.display()
        );
        catalog_hashes.push(actual_hash);
    }
    let (actual_rows, actual_row_groups) =
        planned
            .iter()
            .try_fold((0_u64, 0_u64), |(rows, row_groups), table| {
                let metadata = actual_nt_market_data_metadata(&table.subroot)?;
                Ok((
                    rows.checked_add(metadata.rows)
                        .context("completed actual projected row total overflow")?,
                    row_groups
                        .checked_add(metadata.row_groups)
                        .context("completed actual projected row-group total overflow")?,
                ))
            })?;
    let expected_rows = planned.iter().try_fold(0_u64, |rows, table| {
        rows.checked_add(
            u64::try_from(table.table.rows_len())
                .context("canonical row count does not fit u64")?,
        )
        .context("canonical row total overflow")
    })?;
    ensure!(
        actual_rows == expected_rows,
        "completed actual projected Parquet metadata rows {actual_rows} do not match canonical rows {expected_rows}"
    );
    ensure!(
        actual_row_groups == inputs.projected_row_groups,
        "completed actual projected row groups {actual_row_groups} do not match expected {}",
        inputs.projected_row_groups
    );
    inputs.work_budget.verify_actual_row_groups(
        actual_row_groups,
        OperatorWorkBudgetStage::CatalogProjection,
    )?;
    let index_records = validate_conversion_tables_index(inputs.output_dir, &conversion_manifest)?;
    if planned.len() > 1 {
        let records = index_records.as_deref().with_context(|| {
            format!(
                "completed multi-table conversion is missing {CONVERSION_TABLES_FILE} \
                 ({} tables)",
                planned.len()
            )
        })?;
        let expected_records: Vec<ConversionTableRecord> = planned
            .iter()
            .zip(catalog_hashes.iter())
            .map(|(table, hash)| table.record(hash.clone()))
            .collect();
        ensure!(
            records.len() == expected_records.len(),
            "completed {CONVERSION_TABLES_FILE} has {} records, expected {}",
            records.len(),
            expected_records.len()
        );
        for expected in &expected_records {
            ensure!(
                records.contains(expected),
                "completed {CONVERSION_TABLES_FILE} is missing record {expected:?}"
            );
        }
    } else {
        ensure!(
            index_records.is_none(),
            "single-table conversion must not carry {CONVERSION_TABLES_FILE}"
        );
    }

    // Bind, validate, window-check, then gate 5 once.
    let (local_manifest, bound_indices) = bind_catalog_inputs(&spec.manifest, &planned)?;
    validate_local_run_manifest(&local_manifest, accepted)?;
    assert_tables_overlap_window(&local_manifest, &planned)?;
    let primary_index = *bound_indices
        .first()
        .context("manifest must declare at least one catalog input")?;
    let primary = &planned[primary_index];
    ensure!(
        catalog_hashes[primary_index] == inputs.primary_catalog_hash,
        "completed primary catalog hash mismatch: expected {:?}, got {:?}",
        inputs.primary_catalog_hash,
        catalog_hashes[primary_index]
    );
    let artifact_uris = multi_artifact_uris(&spec.manifest, primary);
    ensure!(
        conversion_manifest.output_catalog_uri == artifact_uris.nt_catalog_uri,
        "completed conversion output_catalog_uri does not match current run manifest"
    );
    let totals = rows_by_nt_data_type(&planned)?;
    ensure!(
        totals == conversion_manifest.effective_catalog_rows_by_nt_data_type(),
        "completed conversion per-data-type rows {totals:?} do not match conversion manifest {:?}",
        conversion_manifest.effective_catalog_rows_by_nt_data_type()
    );
    let (event_count_ledger_hash, selected_asset_ids_hash) =
        multi_selector_provenance(spec, &planned)?;

    let nt_run = run_nt_backtest_node_guarded(&local_manifest, inputs.work_budget)?;
    let nt_result = nt_run.result;
    let config_override_report = nt_run.config_override_report;
    let run_guard_report = nt_run.run_guard_report;
    let window_start = window_bound_nanos("start_time", local_manifest.start_time)?;
    let window_end = window_bound_nanos("end_time", local_manifest.end_time)?;
    let mut expected = 0usize;
    for table in &planned {
        expected += table
            .table
            .windowed_count(window_start, window_end)
            .context("compute expected engine iterations for projected table")?;
    }
    if let Some(reason) = iterations_mismatch(nt_result.iterations, expected) {
        anyhow::bail!("backtest did not consume the accepted data: {reason}");
    }

    let mut claim_limits = accepted.result_contract_claim_limits();
    claim_limits.extend(nt_extension_surface_claim_limits(&local_manifest)?);
    let mut contract = build_result_contract(ResultContractInputs {
        run_id: &local_manifest.run_id,
        source_proof_id: &accepted.source_proof_id,
        source_proof_version: accepted.source_proof_version,
        manifest_hash: &inputs.contract_manifest_hash,
        acceptance_mode: accepted.acceptance_mode,
        accepted_by: &accepted.accepted_by,
        accepted_at: &accepted.accepted_at,
        accepted_object_sha256: &accepted.accepted_object_sha256,
        converter_identity: &conversion_manifest.fingerprint.converter_identity,
        converter_version: &conversion_manifest.fingerprint.converter_version,
        converter_config_hash: &conversion_manifest.fingerprint.converter_config_hash,
        conversion_manifest_hash: &inputs.conversion_manifest_hash,
        conversion_checkpoint_hash: &inputs.conversion_checkpoint_hash,
        catalog_hash: &catalog_hashes[primary_index],
        catalog_metadata_hash: &conversion_catalog_metadata_hash,
        event_count_ledger_hash,
        selected_asset_ids_hash,
        strategy: &local_manifest.strategy,
        execution_model: &local_manifest.execution_model,
        venue_queue_position: local_manifest.venue.queue_position,
        catalog_data_types: local_manifest
            .catalog_inputs
            .iter()
            .map(|input| input.data_type.clone())
            .collect(),
        run_purpose: run_purpose_label(&local_manifest),
        market_structure_fixture: market_structure_label(&local_manifest),
        fidelity_class: primary.table.fidelity_class(),
        claim_limits,
        warnings: result_contract_warnings(&nt_result, primary.table.fidelity_class()),
        mechanical_blockers: Vec::new(),
        config_override_report: config_override_report.as_ref(),
        run_guard_report: run_guard_report.as_ref(),
        feed_labels: result_contract_feed_labels(&local_manifest),
        nt_result: &nt_result,
        artifact_uris,
        created_at: &spec.created_at_utc,
    })
    .map_err(|error| anyhow::anyhow!("result contract construction failed: {error}"))?;
    redact_multi_operator_contract(&mut contract, &spec.manifest, &planned);
    let contract = verify_completed_result_contract(&inputs.contract_path, &contract)?;

    atomic_write(
        &inputs.proof_path,
        serde_json::to_string_pretty(&inputs.accepted_proof)
            .context("serialize accepted source proof")?
            .as_bytes(),
    )
    .with_context(|| format!("write {}", inputs.proof_path.display()))?;
    atomic_write(
        &inputs.run_manifest_path,
        serde_json::to_string_pretty(&spec.manifest.to_artifact_manifest()?)
            .context("serialize resolved run manifest")?
            .as_bytes(),
    )
    .with_context(|| format!("write {}", inputs.run_manifest_path.display()))?;

    let conversion_tables_path =
        (planned.len() > 1).then(|| inputs.output_dir.join(CONVERSION_TABLES_FILE));
    let tables = planned
        .iter()
        .zip(catalog_hashes.iter())
        .map(|(table, hash)| ProjectedTableArtifacts {
            table_family: table.table.table_family().to_string(),
            nt_instrument_id: table.nt_instrument_id.clone(),
            data_type: table.table.nt_data_type().to_string(),
            bar_spec: table.bar_spec.clone(),
            subroot_relative: table.subroot_relative.clone(),
            subroot: table.subroot.clone(),
            canonical_relative: table.canonical_relative.clone(),
            canonical_path: table.canonical_path.clone(),
            rows: table.table.rows_len(),
            catalog_hash: hash.clone(),
        })
        .collect();

    Ok(MultiTableRunArtifacts {
        verified_sha256: inputs.verified_sha256,
        accepted_source_proof: inputs.accepted_proof,
        proof_path: inputs.proof_path,
        contract_path: inputs.contract_path,
        run_manifest_path: inputs.run_manifest_path,
        conversion_manifest_path: inputs.conversion_manifest_path,
        conversion_checkpoint_path: inputs.conversion_checkpoint_path,
        catalog_metadata_path: inputs.catalog_metadata_path,
        conversion_tables_path,
        tables,
        conversion_checkpoint,
        conversion_manifest,
        conversion_catalog_metadata,
        conversion_checkpoint_hash: inputs.conversion_checkpoint_hash,
        conversion_manifest_hash: inputs.conversion_manifest_hash,
        nt_result,
        contract,
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
    run_from_run_spec_and_publish_with_resolved_storage_options_guarded(
        spec,
        object_bytes,
        output_dir,
        options,
        storage_options,
        &OperatorWorkBudgetGuard::unbounded(),
    )
}

pub fn run_from_run_spec_and_publish_with_resolved_storage_options_guarded(
    spec: &RunSpec,
    object_bytes: &[u8],
    output_dir: &Path,
    options: PublishOptions,
    storage_options: Option<&BTreeMap<String, String>>,
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<PublishedRunArtifacts> {
    let mut run = if options.prove_published_catalog {
        run_from_run_spec_pending_guarded(spec, object_bytes, output_dir, work_budget)?
    } else {
        run_from_run_spec_guarded(spec, object_bytes, output_dir, work_budget)?
    };
    let (published_artifacts, published_catalog_proof) = if options.prove_published_catalog {
        let _phase_one_artifacts = publish_output_artifacts_with_storage_options_excluding(
            output_dir,
            &spec.manifest.output_prefix,
            storage_options,
            &[
                CATALOG_METADATA_FILE,
                RESULT_CONTRACT_FILE,
                crate::conversion_boundary::CONVERSION_CHECKPOINT_FILE,
            ],
            work_budget,
        )?;
        let proof =
            prove_published_catalog_consumption(spec, &run.output, storage_options, work_budget)
                .context("published catalog proof failed")?;
        write_published_catalog_proof(output_dir, &mut run, &proof, work_budget)?;
        let published_artifacts = publish_completed_output_with_storage_options(
            output_dir,
            &spec.manifest.output_prefix,
            storage_options,
            work_budget,
        )?;
        (published_artifacts, Some(proof))
    } else {
        (
            publish_completed_output_with_storage_options(
                output_dir,
                &spec.manifest.output_prefix,
                storage_options,
                work_budget,
            )?,
            None,
        )
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
    publish_completed_output_with_storage_options(
        output_dir,
        output_prefix,
        None,
        &OperatorWorkBudgetGuard::unbounded(),
    )
}

fn publish_output_artifacts_with_storage_options_excluding(
    output_dir: &Path,
    output_prefix: &str,
    storage_options: Option<&BTreeMap<String, String>>,
    excluded_relative_paths: &[&str],
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<Vec<PublishedArtifact>> {
    work_budget.check_deadline(OperatorWorkBudgetStage::Publish)?;
    ensure!(
        output_dir.is_dir(),
        "output directory does not exist: {}",
        output_dir.display()
    );
    let mut files = Vec::new();
    for path in collect_output_files(output_dir)? {
        // Fail loud on a path-strip error: a file we cannot relativize must not be
        // silently included in (or dropped from) the publish set.
        let relative = artifact_relative_path(output_dir, &path)?;
        if !excluded_relative_paths.contains(&relative.as_str()) {
            files.push(path);
        }
    }
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
        work_budget,
    )
}

fn publish_completed_output_with_storage_options(
    output_dir: &Path,
    output_prefix: &str,
    storage_options: Option<&BTreeMap<String, String>>,
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<Vec<PublishedArtifact>> {
    work_budget.check_deadline(OperatorWorkBudgetStage::Publish)?;
    ensure!(
        output_dir.is_dir(),
        "output directory does not exist: {}",
        output_dir.display()
    );
    let mut files = collect_output_files(output_dir)?;
    let checkpoint_path = output_dir.join(CONVERSION_CHECKPOINT_FILE);
    let checkpoint_index = files
        .iter()
        .position(|path| path == &checkpoint_path)
        .context("completed output is missing its conversion checkpoint")?;
    let checkpoint: ConversionCheckpoint = read_json_artifact(&checkpoint_path)?;
    checkpoint.validate_for(&checkpoint.fingerprint)?;
    ensure!(
        checkpoint.stage == ConversionCheckpointStage::Completed,
        "published output conversion checkpoint is not completed"
    );
    files.remove(checkpoint_index);

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
    let checkpoint_object_path =
        ObjectPath::from(published_object_key(&base_path, CONVERSION_CHECKPOINT_FILE));
    let checkpoint_bytes = fs::read(&checkpoint_path)
        .with_context(|| format!("read {}", checkpoint_path.display()))?;

    work_budget.check_deadline(OperatorWorkBudgetStage::Publish)?;
    let committed =
        read_published_object(&runtime, object_store.as_ref(), &checkpoint_object_path)?;
    work_budget.check_deadline(OperatorWorkBudgetStage::Publish)?;
    if let Some(existing_checkpoint) = committed {
        ensure!(
            existing_checkpoint.as_ref() == checkpoint_bytes.as_slice(),
            "published completion checkpoint already exists under {output_prefix} with different bytes"
        );
        files.push(checkpoint_path);
        return verify_published_files_exact(
            output_dir,
            &files,
            output_prefix,
            &base_path,
            &runtime,
            object_store.as_ref(),
            work_budget,
        );
    }

    let mut published = publish_selected_artifacts_with_storage_options(
        output_dir,
        &files,
        output_prefix,
        storage_options,
        work_budget,
    )?;

    work_budget.check_deadline(OperatorWorkBudgetStage::Publish)?;
    let raced_checkpoint =
        read_published_object(&runtime, object_store.as_ref(), &checkpoint_object_path)?;
    work_budget.check_deadline(OperatorWorkBudgetStage::Publish)?;
    if let Some(existing_checkpoint) = raced_checkpoint {
        ensure!(
            existing_checkpoint.as_ref() == checkpoint_bytes.as_slice(),
            "published completion checkpoint won a create race under {output_prefix} with different bytes"
        );
        files.push(checkpoint_path);
        return verify_published_files_exact(
            output_dir,
            &files,
            output_prefix,
            &base_path,
            &runtime,
            object_store.as_ref(),
            work_budget,
        );
    }

    verify_published_files_exact(
        output_dir,
        &files,
        output_prefix,
        &base_path,
        &runtime,
        object_store.as_ref(),
        work_budget,
    )?;
    let permit = work_budget.authorize_commit(OperatorWorkBudgetStage::Publish)?;
    let created = commit_remote_checkpoint(
        &runtime,
        object_store.as_ref(),
        &checkpoint_object_path,
        &checkpoint_bytes,
        permit,
    )
    .with_context(|| format!("publish completion checkpoint to {output_prefix}"))?;
    if !created {
        files.push(checkpoint_path);
        return verify_published_files_exact_after_commit(
            output_dir,
            &files,
            output_prefix,
            &base_path,
            &runtime,
            object_store.as_ref(),
        );
    }
    published.push(published_artifact_description(
        &checkpoint_path,
        output_dir,
        output_prefix,
        &checkpoint_bytes,
    )?);
    Ok(published)
}

fn commit_remote_checkpoint(
    runtime: &tokio::runtime::Runtime,
    object_store: &dyn ObjectStore,
    checkpoint_path: &ObjectPath,
    checkpoint_bytes: &[u8],
    _permit: OperatorWorkBudgetCommitPermit,
) -> Result<bool> {
    match runtime.block_on(object_store.put_opts(
        checkpoint_path,
        Bytes::copy_from_slice(checkpoint_bytes).into(),
        PutMode::Create.into(),
    )) {
        Ok(_) => Ok(true),
        Err(ObjectStoreError::AlreadyExists { .. })
        | Err(ObjectStoreError::Precondition { .. }) => {
            let existing = read_published_object(runtime, object_store, checkpoint_path)?
                .context("published completion checkpoint disappeared after create conflict")?;
            ensure!(
                existing.as_ref() == checkpoint_bytes,
                "published completion checkpoint won a create race with different bytes"
            );
            Ok(false)
        }
        Err(error) => Err(error).context("create published completion checkpoint"),
    }
}

fn verify_published_files_exact(
    output_dir: &Path,
    files: &[PathBuf],
    output_prefix: &str,
    base_path: &str,
    runtime: &tokio::runtime::Runtime,
    object_store: &dyn ObjectStore,
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<Vec<PublishedArtifact>> {
    verify_published_files_exact_inner(
        output_dir,
        files,
        output_prefix,
        base_path,
        runtime,
        object_store,
        Some(work_budget),
    )
}

fn verify_published_files_exact_after_commit(
    output_dir: &Path,
    files: &[PathBuf],
    output_prefix: &str,
    base_path: &str,
    runtime: &tokio::runtime::Runtime,
    object_store: &dyn ObjectStore,
) -> Result<Vec<PublishedArtifact>> {
    verify_published_files_exact_inner(
        output_dir,
        files,
        output_prefix,
        base_path,
        runtime,
        object_store,
        None,
    )
}

fn verify_published_files_exact_inner(
    output_dir: &Path,
    files: &[PathBuf],
    output_prefix: &str,
    base_path: &str,
    runtime: &tokio::runtime::Runtime,
    object_store: &dyn ObjectStore,
    work_budget: Option<&OperatorWorkBudgetGuard>,
) -> Result<Vec<PublishedArtifact>> {
    let mut verified = Vec::with_capacity(files.len());
    for local_path in files {
        let relative = artifact_relative_path(output_dir, local_path)?;
        let object_path = ObjectPath::from(published_object_key(base_path, &relative));
        let bytes =
            fs::read(local_path).with_context(|| format!("read {}", local_path.display()))?;
        if let Some(work_budget) = work_budget {
            work_budget.check_deadline(OperatorWorkBudgetStage::Publish)?;
        }
        let existing = read_published_object(runtime, object_store, &object_path)?
            .with_context(|| format!("published artifact {relative} is missing"))?;
        ensure!(
            existing.as_ref() == bytes.as_slice(),
            "published artifact {relative} already exists under {output_prefix} with different bytes"
        );
        if let Some(work_budget) = work_budget {
            work_budget.check_deadline(OperatorWorkBudgetStage::Publish)?;
        }
        verified.push(published_artifact_description(
            local_path,
            output_dir,
            output_prefix,
            &bytes,
        )?);
    }
    Ok(verified)
}

fn published_artifact_description(
    local_path: &Path,
    output_dir: &Path,
    output_prefix: &str,
    bytes: &[u8],
) -> Result<PublishedArtifact> {
    let relative = artifact_relative_path(output_dir, local_path)?;
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    Ok(PublishedArtifact {
        local_path: local_path.to_path_buf(),
        published_uri: format!("{}/{relative}", output_prefix.trim_end_matches('/')),
        bytes: bytes.len() as u64,
        sha256: hex::encode(hasher.finalize()),
    })
}

fn publish_selected_artifacts_with_storage_options(
    output_dir: &Path,
    files: &[PathBuf],
    output_prefix: &str,
    storage_options: Option<&BTreeMap<String, String>>,
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<Vec<PublishedArtifact>> {
    work_budget.check_deadline(OperatorWorkBudgetStage::Publish)?;
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
    let completion_path =
        ObjectPath::from(published_object_key(&base_path, CONVERSION_CHECKPOINT_FILE));

    let mut targets = Vec::with_capacity(files.len());
    for local_path in files {
        let relative = artifact_relative_path(output_dir, local_path)?;
        ensure!(
            relative != CONVERSION_CHECKPOINT_FILE,
            "completion checkpoint must use the dedicated checkpoint-last publisher"
        );
        let object_path = ObjectPath::from(published_object_key(&base_path, &relative));
        targets.push((local_path, relative, object_path));
    }
    work_budget.check_deadline(OperatorWorkBudgetStage::Publish)?;
    let committed =
        read_published_object(&runtime, object_store.as_ref(), &completion_path)?.is_some();
    work_budget.check_deadline(OperatorWorkBudgetStage::Publish)?;

    let mut published = Vec::with_capacity(targets.len());
    for (local_path, relative, object_path) in targets {
        let bytes =
            fs::read(local_path).with_context(|| format!("read {}", local_path.display()))?;
        work_budget.check_deadline(OperatorWorkBudgetStage::Publish)?;
        match read_published_object(&runtime, object_store.as_ref(), &object_path)? {
            Some(existing) => ensure!(
                existing.as_ref() == bytes.as_slice(),
                "published artifact {relative} already exists under {output_prefix} with different bytes"
            ),
            None => {
                ensure!(
                    !committed,
                    "published artifact {relative} is missing under committed prefix {output_prefix}"
                );
                ensure!(
                    read_published_object(&runtime, object_store.as_ref(), &completion_path,)?
                        .is_none(),
                    "published completion checkpoint already exists under {output_prefix}; refusing to fill an object under a committed prefix"
                );
                work_budget.check_deadline(OperatorWorkBudgetStage::Publish)?;
                let put_result = runtime.block_on(object_store.put_opts(
                    &object_path,
                    Bytes::from(bytes.clone()).into(),
                    PutMode::Create.into(),
                ));
                match put_result {
                    Ok(_) => {}
                    Err(ObjectStoreError::AlreadyExists { .. })
                    | Err(ObjectStoreError::Precondition { .. }) => {
                        let existing = read_published_object(
                            &runtime,
                            object_store.as_ref(),
                            &object_path,
                        )?
                        .with_context(|| {
                            format!(
                                "published artifact {relative} disappeared after create conflict"
                            )
                        })?;
                        ensure!(
                            existing.as_ref() == bytes.as_slice(),
                            "published artifact {relative} won a create race with different bytes"
                        );
                    }
                    Err(error) => {
                        return Err(error).with_context(|| {
                            format!("publish artifact {relative} to {output_prefix}")
                        });
                    }
                }
            }
        }
        work_budget.check_deadline(OperatorWorkBudgetStage::Publish)?;
        published.push(published_artifact_description(
            local_path,
            output_dir,
            output_prefix,
            &bytes,
        )?);
    }

    Ok(published)
}

fn read_published_object(
    runtime: &tokio::runtime::Runtime,
    object_store: &dyn ObjectStore,
    object_path: &ObjectPath,
) -> Result<Option<Bytes>> {
    let object = match runtime.block_on(object_store.get(object_path)) {
        Ok(object) => object,
        Err(ObjectStoreError::NotFound { .. }) => return Ok(None),
        Err(error) => return Err(error).context("read published object"),
    };
    runtime
        .block_on(object.bytes())
        .map(Some)
        .context("read published object bytes")
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
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<PublishedCatalogProof> {
    work_budget.check_deadline(OperatorWorkBudgetStage::Backtest)?;
    let (manifest, catalog_uri) = published_catalog_manifest(spec, storage_options)?;
    let nt_result = run_nt_backtest_node_guarded(&manifest, work_budget)?.result;
    let expected_iterations = local_output.nt_result.iterations;
    ensure!(
        nt_result.iterations == expected_iterations,
        "published catalog BacktestNode iterations {} did not match local verified iterations {}",
        nt_result.iterations,
        expected_iterations
    );
    let catalog_input = manifest.single_catalog_input().map_err(|error| {
        anyhow::anyhow!("published catalog proof requires one catalog input: {error}")
    })?;
    let direct_s3_catalog_access_proven =
        catalog_input.catalog_fs_protocol == "s3" && catalog_uri.starts_with("s3://");
    Ok(PublishedCatalogProof {
        proof_version: "published-catalog-proof.v1".to_string(),
        catalog_uri,
        catalog_fs_protocol: catalog_input.catalog_fs_protocol.clone(),
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
    manifest.artifact_store.storage_options.clear();
    manifest.artifact_store.rust_storage_options.clear();
    manifest.artifact_store.ssm_parameters = None;
    {
        let catalog_input = manifest.single_catalog_input_mut().map_err(|error| {
            anyhow::anyhow!("published catalog manifest requires one catalog input: {error}")
        })?;
        catalog_input.catalog_fs_storage_options.clear();
        catalog_input.catalog_fs_rust_storage_options.clear();
        if let Some(local_path) = catalog_uri.strip_prefix("file://") {
            catalog_input.catalog_path = local_path.to_string();
            catalog_input.catalog_fs_protocol = CATALOG_FS_PROTOCOL_NONE.to_string();
        } else if let Some((protocol, path)) = catalog_uri.split_once("://") {
            catalog_input.catalog_path = path.to_string();
            catalog_input.catalog_fs_protocol = protocol.to_string();
            catalog_input.catalog_fs_rust_storage_options =
                storage_options.cloned().unwrap_or_default();
        } else {
            catalog_input.catalog_path = catalog_uri.clone();
            catalog_input.catalog_fs_protocol = CATALOG_FS_PROTOCOL_NONE.to_string();
        }
    }
    Ok((manifest, catalog_uri))
}

fn write_published_catalog_proof(
    output_dir: &Path,
    run: &mut RunArtifacts,
    proof: &PublishedCatalogProof,
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<()> {
    work_budget.check_deadline(OperatorWorkBudgetStage::Finalize)?;
    write_conversion_checkpoint(
        output_dir,
        &ConversionCheckpoint::started(
            run.output.conversion_checkpoint.fingerprint.clone(),
            run.output.conversion_checkpoint.updated_at.clone(),
        ),
    )?;
    let proof_path = output_dir.join(PUBLISHED_CATALOG_PROOF_FILE);
    work_budget.check_deadline(OperatorWorkBudgetStage::Finalize)?;
    atomic_write(
        &proof_path,
        serde_json::to_string_pretty(proof)
            .context("serialize published catalog proof")?
            .as_bytes(),
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
    run.output.contract.catalog_metadata_hash = run
        .output
        .conversion_catalog_metadata
        .content_hash()
        .context("hash updated catalog metadata")?;
    run.output
        .contract
        .validate()
        .map_err(|error| anyhow::anyhow!("updated result contract rejected: {error}"))?;
    work_budget.check_deadline(OperatorWorkBudgetStage::Finalize)?;
    write_pending_conversion_artifacts(
        output_dir,
        &run.output.conversion_manifest,
        &run.output.conversion_catalog_metadata,
    )?;
    atomic_write(
        &run.contract_path,
        serde_json::to_string_pretty(&run.output.contract)
            .context("serialize updated result contract")?
            .as_bytes(),
    )
    .with_context(|| format!("write {}", run.contract_path.display()))?;
    work_budget.check_deadline(OperatorWorkBudgetStage::Finalize)?;
    write_completed_conversion_artifacts_guarded(
        output_dir,
        &run.output.conversion_manifest,
        &run.output.conversion_checkpoint,
        &run.output.conversion_catalog_metadata,
        work_budget,
    )?;
    Ok(())
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
    use std::{
        io::{Cursor, Write},
        sync::{Arc, Mutex},
        time::Duration,
    };

    use crate::hashing::sha256_hex;

    use flate2::{Compression, write::GzEncoder};

    use super::*;
    use crate::canonical_trades::{
        CsvTimestampUnit, FUNDING_RATES_TRANSFORM_IDENTITY, FUNDING_RATES_TRANSFORM_VERSION,
        REGISTERED_SOURCE_ADAPTERS, RawPayloadConfig, RawPayloadContainer,
    };
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
    const COMMITTED_BINANCE_RUN_SPEC: &str = include_str!(
        "../../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-run-spec.binance-bnbusdc-2026-03-01.toml"
    );
    const COMMITTED_BINANCE_ACCEPTED_PROOF: &str = include_str!(
        "../../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-accepted-source-proof.binance-bnbusdc-2026-03-01.json"
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

    fn write_test_completed_checkpoint(output_dir: &Path) -> PathBuf {
        let checkpoint = ConversionCheckpoint::completed(
            ConversionFingerprint {
                source_proof_id: "source-proof".to_string(),
                source_proof_version: 1,
                accepted_object_sha256: "accepted-object".to_string(),
                converter_identity: "converter".to_string(),
                converter_version: "1".to_string(),
                converter_config_hash: "converter-config".to_string(),
            },
            1,
            "catalog-hash",
            "2026-07-15T00:00:00Z",
        );
        write_conversion_checkpoint(output_dir, &checkpoint).expect("write completed checkpoint")
    }

    fn test_publish_prefix(published_root: &Path) -> String {
        format!(
            "file://{}/backtests/published-run",
            published_root.display()
        )
    }

    fn test_work_budget(
        max_source_rows: u64,
        max_projected_row_groups: u64,
    ) -> OperatorWorkBudgetGuard {
        OperatorWorkBudgetGuard::new(crate::operator_work_budget::OperatorWorkBudget::Backfill(
            crate::backfill_execution_plan::BackfillExecutionWorkBudget {
                max_source_rows,
                max_projected_row_groups,
                max_wall_seconds: 60,
                require_object_selection_metadata: false,
            },
        ))
        .expect("test work budget")
    }

    #[derive(Default)]
    struct TestWorkBudgetClock {
        now: Mutex<Duration>,
    }

    impl crate::operator_work_budget::OperatorWorkBudgetClock for TestWorkBudgetClock {
        fn now(&self) -> Duration {
            *self.now.lock().expect("clock mutex")
        }
    }

    impl TestWorkBudgetClock {
        fn set(&self, now: Duration) {
            *self.now.lock().expect("clock mutex") = now;
        }
    }
    /// The committed run-spec, with the accepted-object hash rebound to a locally
    /// reproducible synthetic object (the real staged object is not committed).
    fn run_spec_for(gz_bytes: &[u8]) -> RunSpec {
        let mut spec: RunSpec =
            toml::from_str(COMMITTED_RUN_SPEC).expect("committed run-spec parses");
        assert!(
            spec.source_proof.is_accepted(),
            "committed run-spec must carry an accepted source proof"
        );
        spec.source_bindings_path = PathBuf::from(
            "specs/023-nt-research-analytics-platform/reference/backfill-source-bindings.v1.toml",
        );
        let object_hash = sha256_hex(gz_bytes);
        spec.accepted_object.sha256 = object_hash.clone();
        spec.accepted_object.bytes = gz_bytes.len() as u64;
        spec.source_proof.raw_sample_hash = object_hash;
        spec.converter.raw_payload = RawPayloadConfig {
            container: RawPayloadContainer::CsvGzip,
            max_object_bytes: gz_bytes.len() as u64,
            max_decoded_bytes: 4096,
            zip_member: None,
            max_member_bytes: None,
            member_suffix: None,
        };
        spec
    }

    #[test]
    fn run_spec_rejects_unknown_top_level_fields() {
        let mut value: toml::Value =
            toml::from_str(COMMITTED_RUN_SPEC).expect("committed run-spec parses as TOML");
        value
            .as_table_mut()
            .expect("committed run-spec is a TOML table")
            .insert(
                "unexpected_top_level".to_string(),
                toml::Value::String("must fail closed".to_string()),
            );
        let text = toml::to_string(&value).expect("serialize mutated run-spec");

        let err = toml::from_str::<RunSpec>(&text)
            .expect_err("RunSpec must reject unknown top-level fields");
        let message = err.to_string();
        assert!(
            message.contains("unknown field") && message.contains("unexpected_top_level"),
            "{message}"
        );
    }

    fn pending_run_spec_for(gz_bytes: &[u8]) -> RunSpec {
        let mut spec = run_spec_for(gz_bytes);
        spec.source_proof.status = crate::source_proof::SourceProofStatus::Pending;
        spec.source_proof.acceptance_mode = None;
        spec.source_proof.accepted_by = None;
        spec.source_proof.accepted_at = None;
        spec
    }

    const TEST_TAR_BLOCK: usize = 512;

    fn test_ustar_header(name: &str, size: u64) -> [u8; TEST_TAR_BLOCK] {
        let mut header = [0u8; TEST_TAR_BLOCK];
        let name_bytes = name.as_bytes();
        assert!(name_bytes.len() <= 100, "test member name too long");
        header[0..name_bytes.len()].copy_from_slice(name_bytes);
        header[100..107].copy_from_slice(b"0000644");
        header[108..115].copy_from_slice(b"0000000");
        header[116..123].copy_from_slice(b"0000000");
        let size_field = format!("{size:011o}");
        header[124..135].copy_from_slice(size_field.as_bytes());
        header[135] = b' ';
        header[136..147].copy_from_slice(b"00000000000");
        header[156] = b'0';
        header[257..263].copy_from_slice(b"ustar\0");
        header[263..265].copy_from_slice(b"00");
        header[148..156].copy_from_slice(b"        ");
        let checksum: u32 = header.iter().map(|&byte| u32::from(byte)).sum();
        let checksum_field = format!("{checksum:06o}");
        header[148..154].copy_from_slice(checksum_field.as_bytes());
        header[154] = 0;
        header[155] = b' ';
        header
    }

    fn gzip_tar(members: &[(&str, &[u8])]) -> Vec<u8> {
        let mut tar = Vec::new();
        for (name, data) in members {
            tar.extend_from_slice(&test_ustar_header(name, data.len() as u64));
            tar.extend_from_slice(data);
            let padding = (TEST_TAR_BLOCK - data.len() % TEST_TAR_BLOCK) % TEST_TAR_BLOCK;
            tar.extend(std::iter::repeat_n(0u8, padding));
        }
        tar.extend(std::iter::repeat_n(0u8, TEST_TAR_BLOCK * 2));
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&tar).expect("gzip write");
        encoder.finish().expect("gzip finish")
    }

    fn payload_config(container: RawPayloadContainer) -> RawPayloadConfig {
        RawPayloadConfig {
            container,
            max_object_bytes: 65_536,
            max_decoded_bytes: 64,
            zip_member: None,
            max_member_bytes: None,
            member_suffix: None,
        }
    }

    #[test]
    fn decode_jsonl_text_payload_decodes_within_bound() {
        let config = payload_config(RawPayloadContainer::JsonlText);
        let payload =
            decode_object_payload(&config, b"{\"a\":1}\n{\"a\":2}\n").expect("jsonl text decodes");
        match payload {
            DecodedPayload::Text(text) => assert_eq!(text, "{\"a\":1}\n{\"a\":2}\n"),
            DecodedPayload::TarMembers(_) | DecodedPayload::ParquetBytes(_) => {
                panic!("jsonl text container must decode to a text payload")
            }
        }
    }

    #[test]
    fn decode_jsonl_text_payload_rejects_decoded_bytes_over_bound() {
        let config = payload_config(RawPayloadContainer::JsonlText);
        let oversize = vec![b'x'; 65];
        let err = decode_object_payload(&config, &oversize)
            .err()
            .expect("over-bound jsonl text must be rejected");
        assert!(err.to_string().contains("max_decoded_bytes"), "{err}");
    }

    #[test]
    fn decode_jsonl_gzip_payload_decodes_and_bounds_decoded_bytes() {
        let config = payload_config(RawPayloadContainer::JsonlGzip);
        let payload =
            decode_object_payload(&config, &gzip("{\"a\":1}\n")).expect("jsonl gzip decodes");
        match payload {
            DecodedPayload::Text(text) => assert_eq!(text, "{\"a\":1}\n"),
            DecodedPayload::TarMembers(_) | DecodedPayload::ParquetBytes(_) => {
                panic!("jsonl gzip container must decode to a text payload")
            }
        }

        let oversize_text = "y".repeat(65);
        let err = decode_object_payload(&config, &gzip(&oversize_text))
            .err()
            .expect("over-bound decoded jsonl gzip must be rejected");
        assert!(err.to_string().contains("max_decoded_bytes"), "{err}");
    }

    #[test]
    fn decode_single_jsonl_zip_payload_decodes_with_crc_verification() {
        let mut config = payload_config(RawPayloadContainer::SingleJsonlZip);
        config.max_decoded_bytes = 128;
        let payload = decode_object_payload(&config, &zip_single_csv("book.data", "{\"a\":1}\n"))
            .expect("jsonl zip decodes");
        match payload {
            DecodedPayload::Text(text) => assert_eq!(text, "{\"a\":1}\n"),
            DecodedPayload::TarMembers(_) | DecodedPayload::ParquetBytes(_) => {
                panic!("jsonl zip container must decode to a text payload")
            }
        }

        config.max_decoded_bytes = 4;
        let err = decode_object_payload(&config, &zip_single_csv("book.data", "{\"a\":1}\n"))
            .err()
            .expect("over-bound jsonl zip must be rejected");
        assert!(err.to_string().contains("max_decoded_bytes"), "{err}");
    }

    #[test]
    fn decode_tar_gzip_jsonl_streams_matching_members_in_order() {
        let mut config = payload_config(RawPayloadContainer::TarGzipJsonl);
        config.member_suffix = Some(".jsonl".to_string());
        config.max_member_bytes = Some(64);
        let archive = gzip_tar(&[
            ("a.jsonl", b"{\"seq\":1}\n".as_slice()),
            ("skip.txt", b"not jsonl".as_slice()),
            ("b.jsonl", b"{\"seq\":2}\n".as_slice()),
        ]);
        let payload = decode_object_payload(&config, &archive).expect("tar gzip decodes");
        match payload {
            DecodedPayload::TarMembers(members) => {
                assert_eq!(members.len(), 2, "only matching members are streamed");
                assert_eq!(members[0].name, "a.jsonl");
                assert_eq!(members[0].text, "{\"seq\":1}\n");
                assert_eq!(members[1].name, "b.jsonl");
                assert_eq!(members[1].text, "{\"seq\":2}\n");
            }
            DecodedPayload::Text(_) | DecodedPayload::ParquetBytes(_) => {
                panic!("tar gzip container must decode to tar members")
            }
        }
    }

    #[test]
    fn decode_tar_gzip_jsonl_rejects_member_over_per_member_bound() {
        let mut config = payload_config(RawPayloadContainer::TarGzipJsonl);
        config.member_suffix = Some(".jsonl".to_string());
        config.max_member_bytes = Some(8);
        let archive = gzip_tar(&[("big.jsonl", b"{\"seq\":111111}\n".as_slice())]);
        let err = decode_object_payload(&config, &archive)
            .err()
            .expect("over-bound tar member must be rejected");
        assert!(
            err.to_string().contains("big.jsonl") || err.to_string().contains("member"),
            "{err}"
        );
    }

    #[test]
    fn decode_parquet_file_passes_object_bytes_through() {
        let config = payload_config(RawPayloadContainer::ParquetFile);
        let bytes = b"PAR1synthetic-not-read-here".to_vec();
        let payload = decode_object_payload(&config, &bytes).expect("parquet passthrough");
        match payload {
            DecodedPayload::ParquetBytes(passthrough) => assert_eq!(passthrough, bytes),
            DecodedPayload::Text(_) | DecodedPayload::TarMembers(_) => {
                panic!("parquet container must pass object bytes through")
            }
        }
    }

    #[test]
    fn parquet_object_over_object_cap_is_rejected_before_decode() {
        let config = payload_config(RawPayloadContainer::ParquetFile);
        let err = ensure_object_within_raw_payload_limit(&config, config.max_object_bytes + 1)
            .err()
            .expect("object over max_object_bytes must be rejected");
        assert!(err.to_string().contains("max_object_bytes"), "{err}");
    }

    #[test]
    fn raw_payload_config_requires_tar_member_fields_for_tar_container() {
        let config = payload_config(RawPayloadContainer::TarGzipJsonl);
        let err = validate_raw_payload_config(&config)
            .err()
            .expect("tar container without member_suffix must be rejected");
        assert!(err.to_string().contains("member_suffix"), "{err}");

        let mut with_suffix = payload_config(RawPayloadContainer::TarGzipJsonl);
        with_suffix.member_suffix = Some(".jsonl".to_string());
        let err = validate_raw_payload_config(&with_suffix)
            .err()
            .expect("tar container without max_member_bytes must be rejected");
        assert!(err.to_string().contains("max_member_bytes"), "{err}");
    }

    #[test]
    fn raw_payload_config_rejects_tar_member_fields_on_non_tar_containers() {
        for container in [
            RawPayloadContainer::CsvGzip,
            RawPayloadContainer::CsvText,
            RawPayloadContainer::JsonlText,
            RawPayloadContainer::JsonlGzip,
            RawPayloadContainer::SingleJsonlZip,
            RawPayloadContainer::ParquetFile,
        ] {
            let mut config = payload_config(container);
            config.max_member_bytes = Some(64);
            let err = validate_raw_payload_config(&config)
                .err()
                .expect("max_member_bytes on a non-tar container must be rejected");
            assert!(err.to_string().contains("max_member_bytes"), "{err}");

            let mut config = payload_config(container);
            config.member_suffix = Some(".jsonl".to_string());
            let err = validate_raw_payload_config(&config)
                .err()
                .expect("member_suffix on a non-tar container must be rejected");
            assert!(err.to_string().contains("member_suffix"), "{err}");
        }
    }

    #[test]
    fn raw_payload_config_rejects_zip_member_on_new_containers() {
        for container in [
            RawPayloadContainer::JsonlText,
            RawPayloadContainer::JsonlGzip,
            RawPayloadContainer::SingleJsonlZip,
            RawPayloadContainer::TarGzipJsonl,
            RawPayloadContainer::ParquetFile,
        ] {
            let mut config = payload_config(container);
            if container == RawPayloadContainer::TarGzipJsonl {
                config.member_suffix = Some(".jsonl".to_string());
                config.max_member_bytes = Some(64);
            }
            config.zip_member = Some("member.csv".to_string());
            let err = validate_raw_payload_config(&config)
                .err()
                .expect("zip_member on a non-zip container must be rejected");
            assert!(err.to_string().contains("zip_member"), "{err}");
        }
    }

    #[test]
    fn funding_converter_config_fails_loud_before_raw_decode() {
        let gz = gzip(SAMPLE_CSV);
        let mut spec = run_spec_for(&gz);
        spec.converter.identity = FUNDING_RATES_TRANSFORM_IDENTITY.to_string();
        spec.converter.version = FUNDING_RATES_TRANSFORM_VERSION.to_string();
        spec.converter.raw_payload = payload_config(RawPayloadContainer::JsonlText);

        let err = validate_converter_config(&spec.converter)
            .expect_err("funding raw acquisition must fail at converter preflight");
        let message = err.to_string();
        assert!(
            message.contains("FundingRates") && message.contains("not admissible"),
            "{message}"
        );
    }

    #[test]
    fn funding_run_path_validates_before_raw_decode() {
        // Drive the REAL multi-table run-path (not `validate_converter_config` in
        // isolation) to pin the validate -> decode statement ORDERING in
        // `run_multi_table_from_run_spec`: the `validate_converter_config(...)?`
        // gate must surface before `decode_object_payload(...)` ever runs.
        //
        // `run_spec_for` binds `accepted_object.{bytes,sha256}` to THIS object, so
        // the byte-length / SHA gates that sit between the validate and decode
        // statements all pass for these bytes. `payload_config` (set below) then
        // overrides the decode cap to `max_decoded_bytes = 64` (run_spec_for's own
        // default is 4096) — that 64-byte cap is what the counterfactual relies on.
        // We then make the converter inadmissible (funding adapter + a JSONL
        // container that funding can never consume — funding's admissibility gate
        // returns `false` for every container, see
        // `ensure_container_matches_adapter_kind`).
        let object_bytes = gzip(SAMPLE_CSV);
        let mut spec = run_spec_for(&object_bytes);
        spec.converter.identity = FUNDING_RATES_TRANSFORM_IDENTITY.to_string();
        spec.converter.version = FUNDING_RATES_TRANSFORM_VERSION.to_string();
        spec.converter.raw_payload = payload_config(RawPayloadContainer::JsonlText);

        let dir = tempfile::TempDir::new().unwrap();
        let err = run_multi_table_from_run_spec(&spec, &object_bytes, dir.path())
            .err()
            .expect("funding run-path must fail loud at the converter preflight");
        let message = err.to_string();

        // The "not admissible for adapter kind FundingRates" string is produced
        // ONLY by `ensure_container_matches_adapter_kind` (reached via the
        // `validate_converter_config(...)?` statement). `decode_object_payload`
        // never emits it. Observing it therefore proves validate ran FIRST.
        //
        // Counterfactual: were `decode_object_payload(...)` moved above the
        // validate gate, decoding these gzip-CSV bytes as a `JsonlText` container
        // (read as plain text, capped at `max_decoded_bytes = 64`) would instead
        // surface a distinguishable "max_decoded_bytes" decode error — NOT this
        // admissibility error — so this assertion fails under a reorder.
        assert!(
            message.contains("FundingRates") && message.contains("not admissible"),
            "expected funding admissibility error from the validate gate, got: {message}"
        );
        assert!(
            !message.contains("max_decoded_bytes"),
            "decode error surfaced — decode ran before the validate gate: {message}"
        );
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
        assert_eq!(parsed.execution_model, spec.manifest.execution_model);
        assert_eq!(
            parsed.venue_queue_position,
            Some(spec.manifest.venue.queue_position)
        );
        assert_eq!(
            parsed.catalog_data_types,
            spec.manifest
                .catalog_inputs
                .iter()
                .map(|input| input.data_type.clone())
                .collect::<Vec<_>>()
        );
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
                surface.classification == NtSurfaceClassification::PassThrough
                    && surface.surface == "venue.fill_model"
                    && surface.resolved_value == "None"
            }),
            "{:?}",
            parsed.resolved_nt_surfaces
        );
        assert!(
            parsed.resolved_nt_surfaces.iter().any(|surface| {
                surface.classification == NtSurfaceClassification::UnsupportedForNow
                    && surface.surface == "venue.settlement_prices"
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
    fn run_from_run_spec_rejects_pending_source_proof_before_canonical_work() {
        let gz = gzip(SAMPLE_CSV);
        let spec = pending_run_spec_for(&gz);
        assert!(
            !spec.source_proof.is_accepted(),
            "fixture must exercise a pending source proof"
        );
        let dir = tempfile::TempDir::new().unwrap();

        let err = match run_from_run_spec(&spec, &gz, dir.path()) {
            Ok(_) => panic!("pending source proof must not reach canonical backtest input"),
            Err(err) => err,
        };

        assert!(err.to_string().contains("source proof"), "{err}");
        assert!(
            !dir.path().join(CONVERSION_CHECKPOINT_FILE).exists(),
            "pending proof rejection must happen before conversion checkpoint writes"
        );
        assert!(
            !dir.path().join(CANONICAL_ARTIFACT_FILE).exists(),
            "pending proof rejection must happen before canonical artifact writes"
        );
        assert!(
            !dir.path().join(CATALOG_DIR).exists(),
            "pending proof rejection must happen before NT catalog work"
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
        assert_eq!(parsed.nt_result.machine_id, OPERATOR_ATTESTED_REDACTED);
        assert_eq!(parsed.nt_result.instance_id, OPERATOR_ATTESTED_REDACTED);
        assert_eq!(
            parsed.nt_result.elapsed_time_secs,
            OPERATOR_ATTESTED_ELAPSED_TIME_SECS
        );
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
        for claim_limit in &parsed.claim_limits {
            assert!(
                !claim_limit.contains(dir.path().to_string_lossy().as_ref()),
                "{claim_limit}"
            );
        }
        assert!(parsed.claim_limits.iter().any(|limit| {
            limit.contains("NT pass_through surface catalog.catalog_path")
                && limit.contains(&parsed.artifact_uris.nt_catalog_uri)
        }));
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
            metadata.execution_catalog_uri, metadata.output_catalog_uri,
            "catalog-metadata.json is byte-deterministic: execution_catalog_uri \
             must be the portable output_catalog_uri, never the transient local path"
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
    fn fresh_source_budget_failure_never_writes_a_completed_checkpoint() {
        let gz = gzip(SAMPLE_CSV);
        let spec = run_spec_for(&gz);
        let dir = tempfile::TempDir::new().unwrap();
        let guard = test_work_budget(2, 1);

        let error = run_from_run_spec_guarded(&spec, &gz, dir.path(), &guard)
            .expect_err("third source record must exceed the two-row budget");

        assert!(error.to_string().contains("max_source_rows"), "{error:#}");
        let checkpoint: ConversionCheckpoint =
            read_json_artifact(&dir.path().join(CONVERSION_CHECKPOINT_FILE))
                .expect("started checkpoint remains inspectable");
        assert_ne!(checkpoint.stage, ConversionCheckpointStage::Completed);
    }

    #[test]
    fn completed_output_is_revalidated_against_a_stricter_source_budget() {
        let gz = gzip(SAMPLE_CSV);
        let spec = run_spec_for(&gz);
        let dir = tempfile::TempDir::new().unwrap();
        run_from_run_spec(&spec, &gz, dir.path()).expect("first run completes");
        let guard = test_work_budget(2, 1);

        let error = run_from_run_spec_guarded(&spec, &gz, dir.path(), &guard)
            .expect_err("completed output must not carry across a stricter source budget");

        assert!(error.to_string().contains("max_source_rows"), "{error:#}");
        assert_eq!(guard.source_rows_consumed(), 3);
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
        let contract_json = fs::read_to_string(&first.contract_path).expect("contract");
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
        assert_eq!(
            fs::read_to_string(&second.contract_path).expect("contract"),
            contract_json
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
    fn publish_output_artifacts_accepts_identical_completed_prefix_without_writes() {
        let output_dir = tempfile::TempDir::new().unwrap();
        fs::write(output_dir.path().join("result-contract.json"), b"result").unwrap();
        write_test_completed_checkpoint(output_dir.path());
        let published_root = tempfile::TempDir::new().unwrap();
        let output_prefix = test_publish_prefix(published_root.path());

        let first = publish_output_artifacts(output_dir.path(), &output_prefix)
            .expect("first publish succeeds");
        let second = publish_output_artifacts(output_dir.path(), &output_prefix)
            .expect("identical committed publish is idempotent");

        assert_eq!(first, second);
        assert!(
            second.last().is_some_and(|artifact| artifact
                .published_uri
                .ends_with(CONVERSION_CHECKPOINT_FILE)),
            "completion checkpoint must be reported last"
        );
    }

    #[test]
    fn publish_output_artifacts_resumes_partial_uncommitted_prefix() {
        let output_dir = tempfile::TempDir::new().unwrap();
        fs::write(output_dir.path().join("a.json"), b"a").unwrap();
        fs::write(output_dir.path().join("b.json"), b"b").unwrap();
        write_test_completed_checkpoint(output_dir.path());
        let published_root = tempfile::TempDir::new().unwrap();
        let output_prefix = test_publish_prefix(published_root.path());
        let remote_root = published_root.path().join("backtests/published-run");
        fs::create_dir_all(&remote_root).unwrap();
        fs::write(remote_root.join("a.json"), b"a").unwrap();

        let published = publish_output_artifacts(output_dir.path(), &output_prefix)
            .expect("partial uncommitted prefix resumes");

        assert_eq!(fs::read(remote_root.join("b.json")).unwrap(), b"b");
        assert!(remote_root.join(CONVERSION_CHECKPOINT_FILE).is_file());
        assert!(
            published.last().is_some_and(|artifact| artifact
                .published_uri
                .ends_with(CONVERSION_CHECKPOINT_FILE))
        );
    }

    #[test]
    fn publish_output_artifacts_rejects_conflicting_uncommitted_artifact() {
        let output_dir = tempfile::TempDir::new().unwrap();
        fs::write(
            output_dir.path().join("result-contract.json"),
            b"new-result",
        )
        .unwrap();
        write_test_completed_checkpoint(output_dir.path());
        let published_root = tempfile::TempDir::new().unwrap();
        let output_prefix = test_publish_prefix(published_root.path());
        let existing = published_root
            .path()
            .join("backtests/published-run/result-contract.json");
        fs::create_dir_all(existing.parent().unwrap()).unwrap();
        fs::write(&existing, b"existing-result").unwrap();

        let err = publish_output_artifacts(output_dir.path(), &output_prefix)
            .expect_err("publish must reject pre-existing artifact");

        assert!(err.to_string().contains("already exists"), "{err}");
        assert_eq!(
            fs::read(&existing).unwrap(),
            b"existing-result",
            "existing published artifact must not be overwritten"
        );
        assert!(
            !published_root
                .path()
                .join("backtests/published-run")
                .join(CONVERSION_CHECKPOINT_FILE)
                .exists(),
            "a conflict must not commit the prefix"
        );
    }

    #[test]
    fn publish_output_artifacts_never_repairs_a_committed_prefix() {
        let output_dir = tempfile::TempDir::new().unwrap();
        fs::write(output_dir.path().join("result-contract.json"), b"result").unwrap();
        let local_checkpoint = write_test_completed_checkpoint(output_dir.path());
        let published_root = tempfile::TempDir::new().unwrap();
        let output_prefix = test_publish_prefix(published_root.path());
        let remote_root = published_root.path().join("backtests/published-run");
        fs::create_dir_all(&remote_root).unwrap();
        fs::copy(
            local_checkpoint,
            remote_root.join(CONVERSION_CHECKPOINT_FILE),
        )
        .unwrap();

        let err = publish_output_artifacts(output_dir.path(), &output_prefix)
            .expect_err("committed prefix with a missing object must fail");

        assert!(err.to_string().contains("is missing"), "{err:#}");
        assert!(
            !remote_root.join("result-contract.json").exists(),
            "committed prefix must remain verify-only"
        );
    }

    #[test]
    fn expired_publication_guard_writes_no_remote_completion_artifact() {
        let output_dir = tempfile::TempDir::new().unwrap();
        fs::write(output_dir.path().join("result-contract.json"), b"result").unwrap();
        write_test_completed_checkpoint(output_dir.path());
        let published_root = tempfile::TempDir::new().unwrap();
        let output_prefix = test_publish_prefix(published_root.path());
        let clock = Arc::new(TestWorkBudgetClock::default());
        let guard = OperatorWorkBudgetGuard::with_clock(
            crate::operator_work_budget::OperatorWorkBudget::Backfill(
                crate::backfill_execution_plan::BackfillExecutionWorkBudget {
                    max_source_rows: 1,
                    max_projected_row_groups: 1,
                    max_wall_seconds: 1,
                    require_object_selection_metadata: false,
                },
            ),
            clock.clone(),
        )
        .expect("guard");
        clock.set(Duration::from_secs(1));

        let error = publish_completed_output_with_storage_options(
            output_dir.path(),
            &output_prefix,
            None,
            &guard,
        )
        .expect_err("deadline equality must reject publication");

        assert!(error.to_string().contains("max_wall_seconds"), "{error:#}");
        let remote_root = published_root.path().join("backtests/published-run");
        assert!(!remote_root.join(CONVERSION_CHECKPOINT_FILE).exists());
        assert!(!remote_root.join("result-contract.json").exists());
    }

    #[test]
    fn prove_mode_expiry_after_pending_base_leaves_local_checkpoint_started() {
        let gz = gzip(SAMPLE_CSV);
        let spec = run_spec_for(&gz);
        let output_dir = tempfile::TempDir::new().unwrap();
        let clock = Arc::new(TestWorkBudgetClock::default());
        let guard = OperatorWorkBudgetGuard::with_clock(
            crate::operator_work_budget::OperatorWorkBudget::Backfill(
                crate::backfill_execution_plan::BackfillExecutionWorkBudget {
                    max_source_rows: 10,
                    max_projected_row_groups: 10,
                    max_wall_seconds: 60,
                    require_object_selection_metadata: false,
                },
            ),
            clock.clone(),
        )
        .expect("guard");
        let mut run = run_from_run_spec_pending_guarded(&spec, &gz, output_dir.path(), &guard)
            .expect("pending base run");
        let proof = PublishedCatalogProof {
            proof_version: "published-catalog-proof.v1".to_string(),
            catalog_uri: portable_artifact_uri(&spec.manifest.output_prefix, CATALOG_DIR),
            catalog_fs_protocol: CATALOG_FS_PROTOCOL_NONE.to_string(),
            direct_s3_catalog_access_proven: false,
            expected_iterations: run.output.nt_result.iterations,
            nt_iterations: run.output.nt_result.iterations,
            run_config_id: run.output.nt_result.run_config_id.clone(),
            nt_version: run.output.contract.nt_version.clone(),
            created_at: spec.created_at_utc.clone(),
        };
        clock.set(Duration::from_secs(60));

        let error = write_published_catalog_proof(output_dir.path(), &mut run, &proof, &guard)
            .expect_err("proof finalization must reject deadline equality");

        assert!(error.to_string().contains("max_wall_seconds"), "{error:#}");
        let checkpoint: ConversionCheckpoint =
            read_json_artifact(&output_dir.path().join(CONVERSION_CHECKPOINT_FILE))
                .expect("pending checkpoint");
        assert_eq!(checkpoint.stage, ConversionCheckpointStage::Started);
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
        assert_eq!(manifest.catalog_inputs[0].catalog_fs_protocol, "s3");
        assert_eq!(
            manifest.catalog_inputs[0].catalog_path,
            "example-bucket/backtests/published-run/nt-catalog"
        );
        assert!(
            manifest.catalog_inputs[0]
                .catalog_fs_storage_options
                .is_empty()
        );
        assert_eq!(
            manifest.catalog_inputs[0].catalog_fs_rust_storage_options,
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
        assert!(
            spec.source_proof.is_accepted(),
            "committed run-spec must carry an accepted source proof"
        );
        assert_eq!(
            spec.source_proof.source_proof_id,
            "source-proof-bybit-spot-tick-trades"
        );
        assert_eq!(
            spec.manifest.catalog_inputs[0].nt_instrument_id,
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

        assert!(
            err.to_string().contains("registered source adapter"),
            "{err}"
        );
    }

    #[test]
    fn run_from_run_spec_rejects_non_trade_adapter_kind_before_artifacts() {
        // The single-table trade entry refuses every non-trade kind before any
        // artifact write; those kinds dispatch through run_operator_from_run_spec.
        let gz = gzip(SAMPLE_CSV);
        let mut spec = run_spec_for(&gz);
        let non_trade_adapter = REGISTERED_SOURCE_ADAPTERS
            .iter()
            .find(|adapter| adapter.kind == SourceAdapterKind::CsvNativeBars)
            .expect("test registry must include the CSV native-bars adapter");
        spec.converter.identity = non_trade_adapter.identity.to_string();
        spec.converter.version = non_trade_adapter.version.to_string();
        let dir = tempfile::TempDir::new().unwrap();

        let err = run_from_run_spec(&spec, &gz, dir.path())
            .err()
            .expect("non-trade adapter kind must be rejected by the trade entry");

        assert!(
            err.to_string().contains("single-table trade entry"),
            "{err}"
        );
        assert!(
            err.to_string().contains("run_operator_from_run_spec"),
            "{err}"
        );
        assert!(
            !dir.path().join(CONVERSION_CHECKPOINT_FILE).exists(),
            "trade-entry kind rejection must happen before conversion checkpoint writes"
        );
    }

    #[test]
    fn run_from_run_spec_rejects_kind_container_mismatch_before_artifacts() {
        // A registered non-trade kind paired with a container its dispatcher
        // cannot consume fails converter-config validation before artifacts.
        let gz = gzip(SAMPLE_CSV);
        let mut spec = run_spec_for(&gz);
        let parquet_adapter = REGISTERED_SOURCE_ADAPTERS
            .iter()
            .find(|adapter| adapter.kind == SourceAdapterKind::ParquetEventStreamDeltas)
            .expect("test registry must include the parquet event-stream adapter");
        spec.converter.identity = parquet_adapter.identity.to_string();
        spec.converter.version = parquet_adapter.version.to_string();
        let dir = tempfile::TempDir::new().unwrap();

        let err = run_from_run_spec(&spec, &gz, dir.path())
            .err()
            .expect("kind/container mismatch must be rejected before artifacts");

        assert!(
            err.to_string().contains("not admissible for adapter kind"),
            "{err}"
        );
        assert!(
            !dir.path().join(CONVERSION_CHECKPOINT_FILE).exists(),
            "kind/container rejection must happen before conversion checkpoint writes"
        );
    }

    #[test]
    fn validate_run_spec_rejects_converter_table_family_mismatch_before_artifacts() {
        let gz = gzip(SAMPLE_CSV);
        let mut spec = run_spec_for(&gz);
        let dir = tempfile::TempDir::new().unwrap();
        let registry_path = dir.path().join("source-bindings.toml");
        fs::write(
            &registry_path,
            format!(
                r#"
[[source_binding]]
key = "{}"
venue = "{}"
product_family = "{}"
market_structure_fixture = "perps-spot"
source_uri = "{}"
evidence_state = "owner_archive_backfillable"
table_families = ["trades", "bars"]
"#,
                spec.source_proof.source_binding,
                spec.source_proof.venue,
                spec.source_proof.product_family,
                spec.accepted_object.source_url
            ),
        )
        .expect("write source-binding registry");
        spec.source_bindings_path = registry_path;
        spec.source_proof.table_family = "bars".to_string();

        let err = validate_run_spec_manifest_for_object_hash(
            &spec,
            dir.path(),
            &spec.accepted_object.sha256,
        )
        .expect_err("source adapter must reject a mismatched source table family");

        assert!(err.to_string().contains("adapter"), "{err}");
        assert!(err.to_string().contains("table_family"), "{err}");
        assert!(
            !dir.path().join(CONVERSION_CHECKPOINT_FILE).exists(),
            "table-family rejection must happen before conversion checkpoint writes"
        );
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
        spec.manifest.catalog_inputs[0].nt_instrument_id = "BNBUSDC.BINANCE".to_string();
        let instrument_spec = spec
            .instrument_spec
            .single_mut()
            .expect("single instrument spec")
            .spot_mut()
            .expect("spot instrument spec");
        instrument_spec.nt_instrument_id = "BNBUSDC.BINANCE".to_string();
        instrument_spec.price_increment = "0.00000001".to_string();
        instrument_spec.size_increment = "0.000001".to_string();
        spec.identity
            .single_mut()
            .expect("single instrument identity")
            .nt_instrument_id = "BNBUSDC.BINANCE".to_string();
        spec.manifest.strategy.parameters.insert(
            "bar_type".to_string(),
            "BNBUSDC.BINANCE-1-MINUTE-LAST-INTERNAL".to_string(),
        );
        spec.converter.raw_payload = RawPayloadConfig {
            container: RawPayloadContainer::SingleCsvZip,
            max_object_bytes: zip_bytes.len() as u64,
            max_decoded_bytes: BINANCE_HEADERLESS_CSV.len() as u64,
            zip_member: Some("BNBUSDC-trades-2026-03-01.csv".to_string()),
            max_member_bytes: None,
            member_suffix: None,
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
    fn committed_result_contract_records_catalog_metadata_fixture_claim() {
        let contract: BacktestResultContract =
            serde_json::from_str(COMMITTED_RESULT_CONTRACT).expect("result contract parses");
        let metadata: ConversionCatalogMetadata =
            serde_json::from_str(COMMITTED_CATALOG_METADATA).expect("catalog metadata parses");
        // This committed contract is an operator-attested historical fixture; keep its
        // recorded claim stable instead of recomputing it with current writer semantics.
        assert_eq!(
            contract.catalog_metadata_hash,
            "f82bd70268d1df4163c1746ad79194fc987082e4b6ab9cdc82d6d8275990e882"
        );
        assert_eq!(
            contract.artifact_uris.catalog_metadata_uri,
            "reference://backtesting-vertical-slice/bnbusdc-2026-03-01/catalog-metadata.json"
        );
        assert_eq!(
            metadata.execution_catalog_uri,
            "reference://backtesting-vertical-slice/bnbusdc-2026-03-01/nt-catalog"
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
        assert!(contract.claim_limits.iter().any(|limit| {
            limit.contains("NT pass_through surface venue.name")
                && limit.contains("resolved_value=BYBIT")
        }));
        assert!(contract.claim_limits.iter().any(|limit| {
            limit.contains("NT pass_through surface catalog.catalog_path")
                && limit.contains("s3://bolt-parquet/nt-research-analytics/backtests/")
        }));
        assert!(contract.claim_limits.iter().any(|limit| {
            limit.contains("NT unsupported_for_now surface venue.settlement_prices")
        }));
    }

    #[test]
    fn committed_accepted_proof_deserializes() {
        let proof: SourceProofReport =
            serde_json::from_str(COMMITTED_ACCEPTED_PROOF).expect("accepted proof parses");
        assert!(proof.is_accepted(), "committed proof is accepted");
    }

    #[test]
    fn committed_binance_reference_binds_accepted_source_proof_without_scratch_evidence() {
        assert_no_scratch_evidence(COMMITTED_BINANCE_RUN_SPEC);
        assert_no_scratch_evidence(COMMITTED_BINANCE_ACCEPTED_PROOF);

        let spec: RunSpec =
            toml::from_str(COMMITTED_BINANCE_RUN_SPEC).expect("binance run-spec parses");
        let proof: SourceProofReport = serde_json::from_str(COMMITTED_BINANCE_ACCEPTED_PROOF)
            .expect("binance accepted proof parses");

        proof
            .evaluate_acceptance()
            .expect("binance reference proof still satisfies acceptance invariants");
        assert!(
            spec.source_proof.is_accepted(),
            "binance run-spec must carry the committed accepted source proof"
        );
        assert_eq!(
            spec.source_proof.accepted_by.as_deref(),
            Some(spec.accepted_by.as_str())
        );
        assert_eq!(
            spec.source_proof.accepted_at.as_deref(),
            Some(spec.accepted_at_utc.as_str())
        );
        assert_eq!(
            spec.source_proof, proof,
            "run-spec source proof must be the committed accepted proof"
        );

        let accepted = crate::source_proof::select_accepted_dataset(
            &proof,
            &spec.accepted_object,
            &spec.accepted_object.sha256,
        )
        .expect("committed binance object remains accepted");

        assert_eq!(accepted.source_proof_id, spec.manifest.source_proof_id);
        assert_eq!(
            accepted.source_proof_version,
            spec.manifest.source_proof_version
        );
        assert_eq!(accepted.source_binding, spec.manifest.venue_binding_key);
        assert_eq!(
            spec.manifest.output_prefix,
            format!(
                "{}/backtests/{}",
                spec.manifest.artifact_root.trim_end_matches('/'),
                spec.manifest.run_id
            )
        );
    }

    fn assert_no_scratch_evidence(text: &str) {
        for forbidden in ["scratch://", "placeholder", "not production acceptance"] {
            assert!(
                !text.contains(forbidden),
                "committed reference contains scratch evidence marker {forbidden:?}"
            );
        }
    }
}
