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
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{Cursor, Read, Write},
    mem::size_of,
    path::{Component, Path, PathBuf},
    sync::Arc,
};

#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};

use anyhow::{Context, Result, bail, ensure};
use bytes::Bytes;
use futures_util::StreamExt;
use nautilus_backtest::result::BacktestResult;
use object_store::{ObjectStore, ObjectStoreExt, aws::AmazonS3};
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::atomic_artifact_write::{
    atomic_file_create_or_verify_guarded, open_pinned_regular_file,
};
use crate::{
    artifact_store::{
        ArtifactStoreConfig, BucketVersioningEnabled, CatalogDispatchConfig,
        CatalogProjectionPublicationReceipt, CreateOnlyArtifactWriter, PersistedCatalogProjection,
        PersistedCatalogProjectionObject, ResolvedArtifactRoot, S3ConditionalPutMode,
        ensure_immutable_s3_version_id, hydrate_catalog_projection_from_receipt_guarded,
        persist_catalog_projection_for_source_binding_guarded,
        recover_catalog_projection_from_current_receipt_guarded, required_versioned_create_result,
    },
    canonical_market_data::{
        CanonicalBarsTable, CanonicalFundingRatesTable, CanonicalIndexPricesTable,
        CanonicalMarkPricesTable, CanonicalOrderBookDeltasTable, CanonicalQuotesTable,
        bar_row_materialized_bytes, delta_row_materialized_bytes,
        funding_rate_row_materialized_bytes, mark_price_row_materialized_bytes,
        point_price_row_materialized_bytes, quote_row_materialized_bytes,
    },
    canonical_trades::{
        BAR_TABLE_FAMILY, CanonicalInstrumentIdentity, CanonicalTradesTable, ConverterConfig,
        DELTAS_TABLE_FAMILY, FUNDING_RATES_TABLE_FAMILY, INDEX_PRICES_TABLE_FAMILY,
        MARK_PRICES_TABLE_FAMILY, QUOTE_TABLE_FAMILY, RawPayloadConfig, RawPayloadContainer,
        SourceAdapterKind, TRADE_TABLE_FAMILY, canonical_trade_row_materialized_bytes,
        normalize_registered_bar_converter, normalize_registered_event_stream_delta_converter,
        normalize_registered_funding_converter, normalize_registered_index_converter,
        normalize_registered_jsonl_multi_interval_bar_converter,
        normalize_registered_mark_converter, normalize_registered_order_book_delta_converter,
        normalize_registered_paged_json_bar_converter, normalize_registered_quote_converter,
        normalize_registered_seeded_l2_quote_converter,
        normalize_registered_tar_order_book_delta_converter,
        normalize_registered_tar_seeded_l2_quote_converter, normalize_registered_trade_converter,
        require_registered_source_adapter, require_registered_source_adapter_for_table_family,
        verify_canonical_rows_materialization, verify_parquet_file_trailer_preflight,
        verify_single_parquet_metadata_budget,
    },
    catalog_projection::{
        CatalogInstrumentSpec, CatalogProjection, NT_DATA_TYPE_BAR,
        NT_DATA_TYPE_FUNDING_RATE_UPDATE, NT_DATA_TYPE_INDEX_PRICE_UPDATE,
        NT_DATA_TYPE_MARK_PRICE_UPDATE, NT_DATA_TYPE_ORDER_BOOK_DELTA, NT_DATA_TYPE_QUOTE_TICK,
        NT_DATA_TYPE_TRADE_TICK, actual_nt_market_data_metadata_guarded,
        logical_catalog_hash_guarded, preflight_nt_catalog_parquet_guarded,
        project_canonical_bars_to_catalog_guarded,
        project_canonical_funding_rates_to_catalog_guarded,
        project_canonical_index_to_catalog_guarded, project_canonical_mark_to_catalog_guarded,
        project_canonical_order_book_deltas_to_catalog_guarded,
        project_canonical_quotes_to_catalog_guarded, project_canonical_trades_to_catalog_guarded,
        projected_nt_market_data_row_groups, read_back_bars_guarded,
        read_back_funding_rates_guarded, read_back_index_guarded, read_back_mark_guarded,
        read_back_order_book_deltas_guarded, read_back_quotes_guarded,
        read_back_trade_ticks_guarded, ts_init_nanos,
    },
    conversion_boundary::{
        CATALOG_METADATA_FILE, CONVERSION_CHECKPOINT_FILE, CONVERSION_TABLES_FILE,
        CatalogConsumptionEvidence, CatalogPublicationReceiptIdentity, ConversionCatalogMetadata,
        ConversionCheckpoint, ConversionCheckpointStage, ConversionFingerprint, ConversionManifest,
        ConversionOutputState, ConversionTableRecord, inspect_conversion_output,
        validate_conversion_tables_index, write_completed_conversion_artifacts_guarded,
        write_conversion_tables_index_guarded, write_pending_conversion_artifacts,
    },
    hashing::is_lowercase_sha256_hex,
    nt_catalog_capability::{NtCatalogSsmCredentialResolver, NtCatalogSsmParameterRefs},
    operator_work_budget::{
        CooperativeDeadlineReader, CooperativeDeadlineWriter, ExactSizedObjectBuffer,
        OperatorWorkBudgetGuard, OperatorWorkBudgetStage, cooperative_stable_sort_by,
        deserialize_json_with_budget, guarded_async_operation_outcome,
        guarded_blocking_join_outcome, guarded_operation_outcome, read_file_with_budget,
        serialize_json_to_vec_guarded, sha256_exact_sized_open_file_guarded,
        sha256_hex_with_budget,
    },
    result_contract::{
        BacktestResultContract, ResultArtifactUris, ResultContractInputs, build_result_contract,
    },
    retired_backfill_evidence::resolve_active_backfill_runtime_input,
    run_manifest::{
        BACKTEST_RUN_MANIFEST_ARTIFACT_VERSION, BacktestRunManifestArtifact,
        BacktestingRunManifest, CATALOG_FS_PROTOCOL_NONE, CATALOG_RUN_VIEW_AUTHORITY_FILE,
        CatalogRunViewAuthority, ManifestCatalogInput, SubmittedRunIdentity,
    },
    runner::{
        BacktestRunFinalizeInputs, BacktestRunInputs, BacktestRunOutput, PreparedBacktestRun,
        assert_bar_read_back_matches_guarded, assert_delta_read_back_matches_guarded,
        assert_funding_read_back_matches_guarded, assert_index_read_back_matches_guarded,
        assert_mark_read_back_matches_guarded, assert_quote_read_back_matches_guarded,
        assert_read_back_matches_guarded, assert_time_window_overlaps_data_guarded,
        execute_prepared_backtest, expected_iterations_guarded, iterations_mismatch,
        market_structure_label, mint_local_catalog_run_view_authority_guarded,
        nt_extension_surface_claim_limits, prepare_backtest, result_contract_feed_labels,
        result_contract_warnings, run_nt_backtest_node_guarded, run_purpose_label,
        time_window_excludes_all_data, verify_catalog_run_view_authority_guarded,
        window_bound_nanos,
    },
    source_proof::{
        AcceptedDataset, IngestManifestObjectRecord, SourceBindingRegistry,
        SourceProofFidelityClass, SourceProofReport, select_accepted_dataset_with_registry,
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
/// Final local completion commit binding the exact operator output file set.
pub const OPERATOR_TERMINAL_SEAL_FILE: &str = "operator-terminal-seal.json";
/// Pre-terminal local output-integrity candidate for durable runs.
///
/// This file is deliberately distinct from both the local terminal seal and
/// the remote durable completion manifest. It can prove which local bytes a
/// child finalized, but it never grants durable commit authority by itself.
pub const OPERATOR_DURABLE_OUTPUT_CANDIDATE_SEAL_FILE: &str =
    "operator-durable-output-candidate-seal.json";
/// Published-catalog `BacktestNode` proof artifact filename.
pub const PUBLISHED_CATALOG_PROOF_FILE: &str = "published-catalog-proof.json";
/// Sole remote completion authority for a durable catalog run.
pub const DURABLE_COMPLETION_MANIFEST_FILE: &str = "durable-completion-manifest.json";
const PUBLISHED_CATALOG_PROOF_VERSION: &str = "published-catalog-proof.v2";
const DURABLE_COMPLETION_MANIFEST_VERSION: &str = "durable-completion-manifest.v1";

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
}

/// Opaque authority for the process-local operator path.
///
/// The durable artifact-store path has different crash, publication, and
/// recovery semantics and is admitted only by [`DurableRunDispatcher`].
/// Keeping this wrapper private makes it impossible for another production
/// module to call a local inner path with a durable RunSpec by accident.
#[derive(Clone, Copy)]
struct LocalRunSpec<'a>(&'a RunSpec);

impl<'a> LocalRunSpec<'a> {
    fn new(spec: &'a RunSpec) -> Result<Self> {
        ensure!(
            spec.artifact_store.is_none(),
            "run-spec [artifact_store] must use source_universe_batch_execution; local operator entry points are non-durable"
        );
        Ok(Self(spec))
    }

    fn get(self) -> &'a RunSpec {
        self.0
    }

    /// Explicitly scoped seam for the source-universe unit-test runner. That
    /// runner exercises local conversion before replacing the terminal seal
    /// with a non-authoritative durable candidate. No production build can
    /// construct this exception.
    #[cfg(test)]
    fn for_source_universe_test(spec: &'a RunSpec) -> Self {
        Self(spec)
    }
}

/// Fail-closed preflight shared by local orchestration surfaces which must
/// reject durable RunSpecs before reading objects or creating output.
///
/// # Errors
///
/// Returns an error when `spec` selects durable artifact-store publication.
pub(crate) fn validate_local_run_spec_authority(spec: &RunSpec) -> Result<()> {
    LocalRunSpec::new(spec).map(|_| ())
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
    pub catalog_run_view_authority_path: PathBuf,
    pub canonical_catalog_uri: Option<String>,
    pub persisted_catalog_projection: Option<PersistedCatalogProjection>,
    pub persisted_catalog_objects: Vec<PersistedCatalogProjectionObject>,
    pub output: BacktestRunOutput,
    pub(crate) batch_summary: OperatorRunSummary,
    transient_catalog_root_lease: Option<TransientCatalogRootLease>,
}

/// Exact immutable S3 object identity carried by the durable terminal
/// manifest. A current-key lookup is never an acceptable substitute.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DurableObjectVersionIdentity {
    pub uri: String,
    pub sha256: String,
    pub byte_len: u64,
    pub version_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub e_tag: Option<String>,
}

impl DurableObjectVersionIdentity {
    fn validate(&self, label: &str) -> Result<()> {
        ensure!(!self.uri.trim().is_empty(), "{label} URI must not be empty");
        ensure!(
            is_lowercase_sha256_hex(&self.sha256),
            "{label} SHA-256 must be lowercase hex"
        );
        ensure!(self.byte_len > 0, "{label} byte length must be positive");
        ensure_immutable_s3_version_id(&format!("{label} S3 version ID"), &self.version_id)?;
        if let Some(e_tag) = &self.e_tag {
            ensure!(!e_tag.is_empty(), "{label} ETag must not be empty");
        }
        Ok(())
    }
}

/// Exact-version locator for the remote terminal manifest. Fresh completion
/// returns it directly; crash recovery may derive it only by pinning a
/// non-null version from the deterministic current key and then fully
/// validating that exact version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DurableCompletionLocator {
    pub object: DurableObjectVersionIdentity,
}

impl DurableCompletionLocator {
    /// Validate the exact immutable identity carried by this locator.
    ///
    /// # Errors
    ///
    /// Returns an error when any URI, digest, length, or S3 version identity is
    /// missing or malformed.
    pub fn validate(&self) -> Result<()> {
        self.object.validate("durable completion manifest")
    }
}

/// Scalar receipt for a fully committed or discovered durable run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DurableRunReceipt {
    pub completion: DurableCompletionLocator,
    pub run_id: String,
    pub submitted_manifest_hash: String,
    pub canonical_rows: u64,
    pub nt_catalog_rows: u64,
    pub catalog_hash: String,
}

/// Result of the sole durable execution lane. Exact-current terminal discovery
/// is a separate read-only operation and never enters this write path.
pub(crate) struct DurableRunOutcome {
    #[cfg(test)]
    artifacts: Box<RunArtifacts>,
    receipt: DurableRunReceipt,
}

impl DurableRunOutcome {
    #[cfg(test)]
    #[must_use]
    pub(crate) fn receipt(&self) -> &DurableRunReceipt {
        &self.receipt
    }

    #[cfg(test)]
    #[must_use]
    pub(crate) fn into_artifacts(self) -> Box<RunArtifacts> {
        self.artifacts
    }

    #[must_use]
    pub(crate) fn into_receipt(self) -> DurableRunReceipt {
        self.receipt
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DurableCompletionManifest {
    manifest_version: String,
    run_id: String,
    submitted_manifest_hash: String,
    fingerprint: ConversionFingerprint,
    canonical_rows: u64,
    nt_catalog_rows: u64,
    catalog_hash: String,
    publication_receipt: DurableObjectVersionIdentity,
    result_contract: DurableObjectVersionIdentity,
    catalog_metadata: DurableObjectVersionIdentity,
    published_catalog_proof: DurableObjectVersionIdentity,
    catalog_run_view_authority: DurableObjectVersionIdentity,
}

impl DurableCompletionManifest {
    fn new(
        spec: &RunSpec,
        fingerprint: ConversionFingerprint,
        summary: &OperatorRunSummary,
        publication_receipt: DurableObjectVersionIdentity,
        artifacts: DurableCompletionArtifacts,
    ) -> Self {
        Self {
            manifest_version: DURABLE_COMPLETION_MANIFEST_VERSION.to_string(),
            run_id: spec.manifest.run_id.clone(),
            submitted_manifest_hash: spec.manifest.manifest_hash(),
            fingerprint,
            canonical_rows: summary.canonical_rows,
            nt_catalog_rows: summary.nt_catalog_rows,
            catalog_hash: summary.catalog_hash.clone(),
            publication_receipt,
            result_contract: artifacts.result_contract,
            catalog_metadata: artifacts.catalog_metadata,
            published_catalog_proof: artifacts.published_catalog_proof,
            catalog_run_view_authority: artifacts.catalog_run_view_authority,
        }
    }

    fn validate_for(&self, spec: &RunSpec, fingerprint: &ConversionFingerprint) -> Result<()> {
        ensure!(
            self.manifest_version == DURABLE_COMPLETION_MANIFEST_VERSION,
            "unexpected durable completion manifest version"
        );
        ensure!(
            self.run_id == spec.manifest.run_id
                && self.submitted_manifest_hash == spec.manifest.manifest_hash(),
            "durable completion manifest submitted-run identity mismatch"
        );
        self.fingerprint.validate_against(fingerprint)?;
        ensure!(
            self.canonical_rows > 0 && self.nt_catalog_rows == self.canonical_rows,
            "durable completion manifest row summary is invalid"
        );
        ensure!(
            is_lowercase_sha256_hex(&self.catalog_hash),
            "durable completion manifest catalog hash must be lowercase SHA-256"
        );
        for (label, object) in [
            ("publication receipt", &self.publication_receipt),
            ("result contract", &self.result_contract),
            ("catalog metadata", &self.catalog_metadata),
            ("published catalog proof", &self.published_catalog_proof),
            (
                "catalog run-view authority",
                &self.catalog_run_view_authority,
            ),
        ] {
            object.validate(label)?;
        }
        ensure!(
            self.result_contract.uri
                == portable_artifact_uri(&spec.manifest.output_prefix, RESULT_CONTRACT_FILE)
                && self.catalog_metadata.uri
                    == portable_artifact_uri(&spec.manifest.output_prefix, CATALOG_METADATA_FILE)
                && self.published_catalog_proof.uri
                    == portable_artifact_uri(
                        &spec.manifest.output_prefix,
                        PUBLISHED_CATALOG_PROOF_FILE,
                    )
                && self.catalog_run_view_authority.uri
                    == portable_artifact_uri(
                        &spec.manifest.output_prefix,
                        CATALOG_RUN_VIEW_AUTHORITY_FILE,
                    ),
            "durable completion manifest artifact URI mismatch"
        );
        let unique_uris = [
            self.publication_receipt.uri.as_str(),
            self.result_contract.uri.as_str(),
            self.catalog_metadata.uri.as_str(),
            self.published_catalog_proof.uri.as_str(),
            self.catalog_run_view_authority.uri.as_str(),
        ]
        .into_iter()
        .collect::<BTreeSet<_>>();
        ensure!(
            unique_uris.len() == 5,
            "durable completion manifest object URIs must be distinct"
        );
        Ok(())
    }
}

/// Bind the durable result contract to the exact submitted run, accepted
/// conversion fingerprint, and terminal manifest. This is the single
/// cross-artifact contract check used both before a fresh terminal create and
/// while validating an exact remote terminal during recovery.
fn validate_durable_result_contract_cross_claims(
    contract: &BacktestResultContract,
    spec: &RunSpec,
    fingerprint: &ConversionFingerprint,
    terminal: &DurableCompletionManifest,
) -> Result<()> {
    terminal.validate_for(spec, fingerprint)?;
    contract
        .validate()
        .map_err(|error| anyhow::anyhow!("validate durable result contract: {error}"))?;
    let acceptance_mode = spec
        .source_proof
        .acceptance_mode
        .context("accepted source proof is missing acceptance_mode")?;
    let accepted_by = spec
        .source_proof
        .accepted_by
        .as_deref()
        .context("accepted source proof is missing accepted_by")?;
    let accepted_at = spec
        .source_proof
        .accepted_at
        .as_deref()
        .context("accepted source proof is missing accepted_at")?;
    ensure!(
        contract.run_id == spec.manifest.run_id
            && contract.manifest_hash == spec.manifest.manifest_hash()
            && contract.nt_version == spec.manifest.resolved_nt_version
            && contract.created_at == spec.created_at_utc,
        "durable result contract submitted-run identity does not match RunSpec"
    );
    ensure!(
        contract.source_proof_id == fingerprint.source_proof_id
            && contract.source_proof_version == fingerprint.source_proof_version
            && contract.accepted_object_sha256 == fingerprint.accepted_object_sha256
            && contract.converter_identity == fingerprint.converter_identity
            && contract.converter_version == fingerprint.converter_version
            && contract.converter_config_hash == fingerprint.converter_config_hash,
        "durable result contract source/conversion identity does not match the terminal fingerprint"
    );
    ensure!(
        contract.acceptance_mode == acceptance_mode
            && contract.accepted_by == spec.accepted_by
            && contract.accepted_by == accepted_by
            && contract.accepted_at == spec.accepted_at_utc
            && contract.accepted_at == accepted_at,
        "durable result contract acceptance identity does not match RunSpec"
    );
    let expected_catalog_data_types = spec
        .manifest
        .catalog_inputs
        .iter()
        .map(|input| input.data_type.clone())
        .collect::<Vec<_>>();
    ensure!(
        contract.strategy_config_hash == spec.manifest.strategy_config_hash
            && contract.execution_model == spec.manifest.execution_model
            && contract.venue_queue_position == Some(spec.manifest.venue.queue_position)
            && contract.catalog_data_types == expected_catalog_data_types
            && contract.run_purpose == run_purpose_label(&spec.manifest)
            && contract.market_structure_fixture == market_structure_label(&spec.manifest)
            && contract.fidelity_class == spec.source_proof.fidelity_class,
        "durable result contract execution claims do not match RunSpec"
    );
    let expected_selector_hashes = spec
        .selector_provenance
        .as_ref()
        .map(|selector| {
            (
                Some(selector.event_count_ledger_hash.as_str()),
                Some(selector.selected_asset_ids_hash.as_str()),
            )
        })
        .unwrap_or((None, None));
    ensure!(
        contract.event_count_ledger_hash.as_deref() == expected_selector_hashes.0
            && contract.selected_asset_ids_hash.as_deref() == expected_selector_hashes.1,
        "durable result contract selector provenance does not match RunSpec"
    );
    ensure!(
        contract.catalog_hash == terminal.catalog_hash
            && contract.nt_result.iterations == terminal.nt_catalog_rows,
        "durable result contract catalog/row claims do not match terminal summary"
    );
    ensure!(
        contract.artifact_uris.source_proof_uri
            == portable_artifact_uri(&spec.manifest.output_prefix, ACCEPTED_SOURCE_PROOF_FILE)
            && contract.artifact_uris.canonical_table_uri
                == portable_artifact_uri(&spec.manifest.output_prefix, CANONICAL_ARTIFACT_FILE)
            && contract.artifact_uris.catalog_metadata_uri == terminal.catalog_metadata.uri
            && contract.artifact_uris.result_contract_uri == terminal.result_contract.uri
            && contract.artifact_uris.nt_catalog_manifest_uri.as_deref()
                == Some(terminal.publication_receipt.uri.as_str()),
        "durable result contract artifact URIs do not match RunSpec and terminal manifest"
    );
    Ok(())
}

struct DurableCompletionArtifacts {
    result_contract: DurableObjectVersionIdentity,
    catalog_metadata: DurableObjectVersionIdentity,
    published_catalog_proof: DurableObjectVersionIdentity,
    catalog_run_view_authority: DurableObjectVersionIdentity,
}

/// Fully validated scalar receipt prepared before the operator terminal seal
/// is committed. Batch execution can move these fields without any
/// post-terminal validation, hashing, allocation-dependent aggregation, or I/O.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OperatorRunSummary {
    pub canonical_rows: u64,
    pub nt_catalog_rows: u64,
    pub catalog_hash: String,
}

const OPERATOR_TERMINAL_SEAL_VERSION: &str = "operator-terminal-seal.v1";
const OPERATOR_DURABLE_OUTPUT_CANDIDATE_SEAL_VERSION: &str =
    "operator-durable-output-candidate-seal.v1";
const OPERATOR_DURABLE_OUTPUT_CANDIDATE_AUTHORITY_SCOPE: &str =
    "local-output-integrity-only-not-durable-completion";

/// One exact regular file committed by an operator run. The terminal seal is
/// deliberately excluded from this list so it can be written last without a
/// content-hash cycle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct OperatorTerminalSealFile {
    relative_path: String,
    bytes: u64,
    sha256: String,
}

/// Sole local operator-completion authority. A conversion checkpoint can
/// prove that projection finished, but only this final create-only artifact
/// binds the complete local backtest output. It is never batch recovery
/// authority; durable recovery uses exact-current remote terminal discovery.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct OperatorTerminalSeal {
    seal_version: String,
    run_id: String,
    submitted_manifest_hash: String,
    fingerprint: ConversionFingerprint,
    canonical_rows: u64,
    nt_catalog_rows: u64,
    catalog_hash: String,
    files: Vec<OperatorTerminalSealFile>,
    committed_at: String,
}

/// Immutable local byte-set evidence created immediately before the remote
/// terminal publication attempt. The explicit authority scope makes this
/// structurally incapable of masquerading as either local or remote terminal
/// completion.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct OperatorDurableOutputCandidateSeal {
    seal_version: String,
    authority_scope: String,
    run_id: String,
    submitted_manifest_hash: String,
    fingerprint: ConversionFingerprint,
    canonical_rows: u64,
    nt_catalog_rows: u64,
    catalog_hash: String,
    files: Vec<OperatorTerminalSealFile>,
    sealed_at: String,
}

struct OperatorOutputSealContents<'a> {
    role: &'static str,
    run_id: &'a str,
    submitted_manifest_hash: &'a str,
    fingerprint: &'a ConversionFingerprint,
    canonical_rows: u64,
    nt_catalog_rows: u64,
    catalog_hash: &'a str,
    files: &'a [OperatorTerminalSealFile],
    timestamp: &'a str,
    timestamp_field: &'static str,
}

fn validate_operator_output_seal_contents(
    contents: OperatorOutputSealContents<'_>,
    spec: &RunSpec,
    fingerprint: &ConversionFingerprint,
) -> Result<()> {
    ensure!(
        contents.run_id == spec.manifest.run_id,
        "{} run_id mismatch",
        contents.role
    );
    ensure!(
        contents.submitted_manifest_hash == spec.manifest.manifest_hash(),
        "{} submitted manifest hash mismatch",
        contents.role
    );
    contents.fingerprint.validate_against(fingerprint)?;
    ensure!(
        contents.canonical_rows > 0 && contents.nt_catalog_rows == contents.canonical_rows,
        "{} summary row counts are invalid",
        contents.role
    );
    ensure!(
        is_lowercase_sha256_hex(contents.catalog_hash),
        "{} catalog_hash must be lowercase SHA-256",
        contents.role
    );
    ensure!(
        !contents.timestamp.trim().is_empty() && contents.timestamp == spec.created_at_utc,
        "{} {} mismatch",
        contents.role,
        contents.timestamp_field
    );
    ensure!(
        !contents.files.is_empty(),
        "{} exact file set must not be empty",
        contents.role
    );
    let mut previous: Option<&str> = None;
    for file in contents.files {
        ensure_safe_terminal_seal_relative_path(&file.relative_path)?;
        ensure!(
            file.relative_path != OPERATOR_TERMINAL_SEAL_FILE
                && file.relative_path != OPERATOR_DURABLE_OUTPUT_CANDIDATE_SEAL_FILE,
            "{} must exclude every reserved output-seal path from the exact file set",
            contents.role
        );
        ensure!(
            is_lowercase_sha256_hex(&file.sha256),
            "{} file {} has invalid SHA-256",
            contents.role,
            file.relative_path
        );
        ensure!(
            file.bytes > 0,
            "{} file {} must have a positive byte length",
            contents.role,
            file.relative_path
        );
        if let Some(previous) = previous {
            ensure!(
                previous < file.relative_path.as_str(),
                "{} files must be strictly sorted and unique",
                contents.role
            );
        }
        previous = Some(&file.relative_path);
    }
    Ok(())
}

impl OperatorTerminalSeal {
    fn new(
        spec: &RunSpec,
        fingerprint: ConversionFingerprint,
        summary: &OperatorRunSummary,
        files: Vec<OperatorTerminalSealFile>,
    ) -> Self {
        Self {
            seal_version: OPERATOR_TERMINAL_SEAL_VERSION.to_string(),
            run_id: spec.manifest.run_id.clone(),
            submitted_manifest_hash: spec.manifest.manifest_hash(),
            fingerprint,
            canonical_rows: summary.canonical_rows,
            nt_catalog_rows: summary.nt_catalog_rows,
            catalog_hash: summary.catalog_hash.clone(),
            files,
            committed_at: spec.created_at_utc.clone(),
        }
    }

    fn validate_for(&self, spec: &RunSpec, fingerprint: &ConversionFingerprint) -> Result<()> {
        ensure!(
            self.seal_version == OPERATOR_TERMINAL_SEAL_VERSION,
            "unexpected operator terminal seal version: expected {OPERATOR_TERMINAL_SEAL_VERSION:?}, got {:?}",
            self.seal_version
        );
        validate_operator_output_seal_contents(
            OperatorOutputSealContents {
                role: "operator terminal seal",
                run_id: &self.run_id,
                submitted_manifest_hash: &self.submitted_manifest_hash,
                fingerprint: &self.fingerprint,
                canonical_rows: self.canonical_rows,
                nt_catalog_rows: self.nt_catalog_rows,
                catalog_hash: &self.catalog_hash,
                files: &self.files,
                timestamp: &self.committed_at,
                timestamp_field: "committed_at",
            },
            spec,
            fingerprint,
        )
    }

    fn summary(&self) -> OperatorRunSummary {
        OperatorRunSummary {
            canonical_rows: self.canonical_rows,
            nt_catalog_rows: self.nt_catalog_rows,
            catalog_hash: self.catalog_hash.clone(),
        }
    }
}

impl OperatorDurableOutputCandidateSeal {
    fn new(
        spec: &RunSpec,
        fingerprint: ConversionFingerprint,
        summary: &OperatorRunSummary,
        files: Vec<OperatorTerminalSealFile>,
    ) -> Self {
        Self {
            seal_version: OPERATOR_DURABLE_OUTPUT_CANDIDATE_SEAL_VERSION.to_string(),
            authority_scope: OPERATOR_DURABLE_OUTPUT_CANDIDATE_AUTHORITY_SCOPE.to_string(),
            run_id: spec.manifest.run_id.clone(),
            submitted_manifest_hash: spec.manifest.manifest_hash(),
            fingerprint,
            canonical_rows: summary.canonical_rows,
            nt_catalog_rows: summary.nt_catalog_rows,
            catalog_hash: summary.catalog_hash.clone(),
            files,
            sealed_at: spec.created_at_utc.clone(),
        }
    }

    fn validate_for(&self, spec: &RunSpec, fingerprint: &ConversionFingerprint) -> Result<()> {
        ensure!(
            self.seal_version == OPERATOR_DURABLE_OUTPUT_CANDIDATE_SEAL_VERSION,
            "unexpected durable output candidate seal version: expected {OPERATOR_DURABLE_OUTPUT_CANDIDATE_SEAL_VERSION:?}, got {:?}",
            self.seal_version
        );
        ensure!(
            self.authority_scope == OPERATOR_DURABLE_OUTPUT_CANDIDATE_AUTHORITY_SCOPE,
            "durable output candidate seal authority_scope mismatch"
        );
        validate_operator_output_seal_contents(
            OperatorOutputSealContents {
                role: "durable output candidate seal",
                run_id: &self.run_id,
                submitted_manifest_hash: &self.submitted_manifest_hash,
                fingerprint: &self.fingerprint,
                canonical_rows: self.canonical_rows,
                nt_catalog_rows: self.nt_catalog_rows,
                catalog_hash: &self.catalog_hash,
                files: &self.files,
                timestamp: &self.sealed_at,
                timestamp_field: "sealed_at",
            },
            spec,
            fingerprint,
        )
    }

    fn summary(&self) -> OperatorRunSummary {
        OperatorRunSummary {
            canonical_rows: self.canonical_rows,
            nt_catalog_rows: self.nt_catalog_rows,
            catalog_hash: self.catalog_hash.clone(),
        }
    }
}

fn submitted_run_identity_for_spec(spec: &RunSpec) -> Result<SubmittedRunIdentity> {
    SubmittedRunIdentity::new(&spec.manifest, &spec.manifest.manifest_hash())
        .map_err(|error| anyhow::anyhow!(error))
}

fn persist_immutable_local_bytes_guarded(
    path: &Path,
    bytes: &[u8],
    role: &str,
    work_budget: &OperatorWorkBudgetGuard,
    stage: OperatorWorkBudgetStage,
) -> Result<()> {
    work_budget.verify_decoded_bytes(
        u64::try_from(bytes.len())
            .with_context(|| format!("immutable {role} byte length does not fit u64"))?,
        stage,
    )?;
    atomic_file_create_or_verify_guarded(path, work_budget, stage, |file| {
        let mut writer = CooperativeDeadlineWriter::new(file, work_budget, stage);
        writer
            .write_all(bytes)
            .with_context(|| format!("write immutable {role}"))?;
        writer
            .flush()
            .with_context(|| format!("flush immutable {role}"))?;
        Ok(())
    })
    .with_context(|| format!("persist immutable {role} at {}", path.display()))
}

fn persist_catalog_run_view_authority_guarded(
    spec: &RunSpec,
    runtime_manifest: &BacktestingRunManifest,
    authority: &CatalogRunViewAuthority,
    output_dir: &Path,
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<()> {
    let submitted_identity = submitted_run_identity_for_spec(spec)?;
    let stage = OperatorWorkBudgetStage::CatalogProjection;
    let bytes = authority.canonical_bytes_guarded(
        runtime_manifest,
        &submitted_identity,
        work_budget,
        stage,
    )?;
    let expected_sha256 = authority.authority_sha256_guarded(
        runtime_manifest,
        &submitted_identity,
        work_budget,
        stage,
    )?;
    ensure!(
        sha256_hex_with_budget(&bytes, work_budget, stage)? == expected_sha256,
        "catalog run-view authority canonical bytes/hash disagree"
    );
    let path = output_dir.join(CATALOG_RUN_VIEW_AUTHORITY_FILE);
    persist_immutable_local_bytes_guarded(
        &path,
        &bytes,
        "catalog run-view authority",
        work_budget,
        stage,
    )
}

fn load_catalog_run_view_authority_guarded(
    spec: &RunSpec,
    runtime_manifest: &BacktestingRunManifest,
    output_dir: &Path,
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<CatalogRunViewAuthority> {
    let stage = OperatorWorkBudgetStage::CatalogProjection;
    let path = output_dir.join(CATALOG_RUN_VIEW_AUTHORITY_FILE);
    let bytes = read_file_with_budget(&path, work_budget, stage)?;
    let authority: CatalogRunViewAuthority =
        deserialize_json_with_budget(&bytes, work_budget, stage)
            .with_context(|| format!("parse {}", path.display()))?;
    let submitted_identity = submitted_run_identity_for_spec(spec)?;
    let canonical = authority.canonical_bytes_guarded(
        runtime_manifest,
        &submitted_identity,
        work_budget,
        stage,
    )?;
    ensure!(
        bytes == canonical,
        "catalog run-view authority bytes are not canonical"
    );
    Ok(authority)
}

fn load_and_verify_catalog_run_view_authority_guarded(
    spec: &RunSpec,
    runtime_manifest: &BacktestingRunManifest,
    output_dir: &Path,
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<CatalogRunViewAuthority> {
    let authority =
        load_catalog_run_view_authority_guarded(spec, runtime_manifest, output_dir, work_budget)?;
    let submitted_identity = submitted_run_identity_for_spec(spec)?;
    verify_catalog_run_view_authority_guarded(
        runtime_manifest,
        &submitted_identity,
        &authority,
        work_budget,
    )?;
    Ok(authority)
}

impl OperatorRunSummary {
    fn trade(output: &BacktestRunOutput) -> Result<Self> {
        let canonical_rows = u64::try_from(output.canonical_table.rows.len())
            .context("trade canonical row count does not fit u64")?;
        let nt_catalog_rows = u64::try_from(output.read_back_count)
            .context("trade NT catalog row count does not fit u64")?;
        ensure!(
            is_lowercase_sha256_hex(&output.projection.catalog_hash),
            "trade catalog hash is not lowercase SHA-256"
        );
        Ok(Self {
            canonical_rows,
            nt_catalog_rows,
            catalog_hash: output.projection.catalog_hash.clone(),
        })
    }

    fn multi(tables: &[ProjectedTableArtifacts], catalog_hash: &str) -> Result<Self> {
        let canonical_rows = tables.iter().try_fold(0_u64, |total, table| {
            let rows = u64::try_from(table.rows)
                .context("multi-table canonical row count does not fit u64")?;
            total
                .checked_add(rows)
                .context("multi-table canonical row count overflow")
        })?;
        ensure!(
            is_lowercase_sha256_hex(catalog_hash),
            "multi-table catalog hash is not lowercase SHA-256"
        );
        Ok(Self {
            canonical_rows,
            nt_catalog_rows: canonical_rows,
            catalog_hash: catalog_hash.to_string(),
        })
    }
}

#[derive(Debug, PartialEq, Eq)]
struct CatalogInventorySummary {
    data_rows: u64,
    data_row_groups: u64,
    decoded_bytes: u64,
    files: BTreeMap<PathBuf, u64>,
}

fn nt_catalog_data_directory(nt_data_type: &str) -> Result<&'static str> {
    match nt_data_type {
        NT_DATA_TYPE_TRADE_TICK => Ok("trades"),
        NT_DATA_TYPE_BAR => Ok("bars"),
        NT_DATA_TYPE_ORDER_BOOK_DELTA => Ok("order_book_deltas"),
        NT_DATA_TYPE_QUOTE_TICK => Ok("quotes"),
        NT_DATA_TYPE_INDEX_PRICE_UPDATE => Ok("index_prices"),
        NT_DATA_TYPE_MARK_PRICE_UPDATE => Ok("mark_prices"),
        NT_DATA_TYPE_FUNDING_RATE_UPDATE => Ok("funding_rate_update"),
        other => anyhow::bail!("unsupported completed-output NT data type {other:?}"),
    }
}

fn collect_catalog_files_guarded(
    catalog_root: &Path,
    directory: &Path,
    allowed_prefixes: &[PathBuf],
    work_budget: &OperatorWorkBudgetGuard,
    files: &mut BTreeSet<PathBuf>,
) -> Result<()> {
    work_budget.check_deadline(OperatorWorkBudgetStage::CatalogProjection)?;
    let mut entries = fs::read_dir(directory)
        .with_context(|| format!("read completed catalog directory {}", directory.display()))?;
    loop {
        work_budget.check_deadline(OperatorWorkBudgetStage::CatalogProjection)?;
        let Some(entry) = entries.next().transpose().with_context(|| {
            format!("read completed catalog entry under {}", directory.display())
        })?
        else {
            break;
        };
        let path = entry.path();
        let relative = path
            .strip_prefix(catalog_root)
            .with_context(|| format!("derive completed catalog path {}", path.display()))?
            .to_path_buf();
        let file_type = entry
            .file_type()
            .with_context(|| format!("read completed catalog file type {}", path.display()))?;
        ensure!(
            !file_type.is_symlink(),
            "completed catalog contains symlink {}",
            path.display()
        );
        if file_type.is_dir() {
            ensure!(
                allowed_prefixes
                    .iter()
                    .any(|prefix| prefix.starts_with(&relative) || relative.starts_with(prefix)),
                "completed catalog contains unexpected directory {}",
                relative.display()
            );
            collect_catalog_files_guarded(
                catalog_root,
                &path,
                allowed_prefixes,
                work_budget,
                files,
            )?;
            continue;
        }
        ensure!(
            file_type.is_file()
                && path
                    .extension()
                    .is_some_and(|extension| extension == "parquet")
                && allowed_prefixes
                    .iter()
                    .any(|prefix| relative.starts_with(prefix)),
            "completed catalog contains unexpected file {}",
            relative.display()
        );
        ensure!(
            files.insert(relative.clone()),
            "completed catalog enumerated duplicate file {}",
            relative.display()
        );
    }
    Ok(())
}

fn preflight_completed_catalog(
    catalog_root: &Path,
    nt_data_type: &str,
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<CatalogInventorySummary> {
    ensure!(
        catalog_root.is_dir(),
        "completed catalog root is missing: {}",
        catalog_root.display()
    );
    let data_prefix = PathBuf::from("data").join(nt_catalog_data_directory(nt_data_type)?);
    let instrument_prefix = PathBuf::from("data").join("instruments");
    let allowed_prefixes = [instrument_prefix.clone(), data_prefix.clone()];
    let mut files = BTreeSet::new();
    collect_catalog_files_guarded(
        catalog_root,
        catalog_root,
        &allowed_prefixes,
        work_budget,
        &mut files,
    )?;
    let instrument_files = files
        .iter()
        .filter(|path| path.starts_with(&instrument_prefix))
        .count();
    let data_files = files
        .iter()
        .filter(|path| path.starts_with(&data_prefix))
        .count();
    ensure!(
        instrument_files == 1 && data_files == 1 && files.len() == 2,
        "completed catalog {} must contain exactly one instrument Parquet and one {nt_data_type} Parquet; found {instrument_files} instrument, {data_files} data, {} total",
        catalog_root.display(),
        files.len()
    );

    let preflight = preflight_nt_catalog_parquet_guarded(
        catalog_root,
        work_budget,
        OperatorWorkBudgetStage::CatalogProjection,
    )?;
    ensure!(
        preflight.files.len() == files.len()
            && preflight
                .files
                .iter()
                .all(|file| files.contains(&file.relative_path)),
        "completed catalog {} Parquet inventory changed between exact-set traversal and preflight",
        catalog_root.display()
    );
    let decoded_bytes = preflight
        .total_file_bytes
        .checked_add(preflight.total_footer_metadata_bytes)
        .and_then(|total| total.checked_add(preflight.total_uncompressed_bytes))
        .context("completed catalog decoded byte total overflow")?;
    let files = preflight
        .files
        .into_iter()
        .map(|file| (file.relative_path, file.file_bytes))
        .collect::<BTreeMap<_, _>>();
    Ok(CatalogInventorySummary {
        data_rows: preflight.market_data.rows,
        data_row_groups: preflight.market_data.row_groups,
        decoded_bytes,
        files,
    })
}

fn completed_catalog_relative_path(spec: &RunSpec, uri: &str) -> Result<PathBuf> {
    let prefix = spec.manifest.output_prefix.trim_end_matches('/');
    let relative = uri
        .strip_prefix(prefix)
        .and_then(|suffix| suffix.strip_prefix('/'))
        .with_context(|| {
            format!(
                "completed output catalog URI {uri:?} is not under run output prefix {prefix:?}"
            )
        })?;
    let path = PathBuf::from(relative);
    ensure!(
        !path.as_os_str().is_empty()
            && !path.is_absolute()
            && path
                .components()
                .all(|component| matches!(component, Component::Normal(_))),
        "completed output catalog URI has unsafe relative path {relative:?}"
    );
    Ok(path)
}

fn verify_catalog_root_set(
    output_dir: &Path,
    expected_roots: &BTreeSet<PathBuf>,
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<()> {
    for top_level in [CATALOG_DIR, NT_CATALOGS_DIR] {
        let root = output_dir.join(top_level);
        if !root.exists() {
            continue;
        }
        let mut stack = vec![root];
        while let Some(directory) = stack.pop() {
            work_budget.check_deadline(OperatorWorkBudgetStage::CatalogProjection)?;
            for entry in fs::read_dir(&directory)
                .with_context(|| format!("audit completed catalog tree {}", directory.display()))?
            {
                let entry = entry?;
                let path = entry.path();
                let file_type = entry.file_type()?;
                ensure!(
                    !file_type.is_symlink(),
                    "completed catalog tree contains symlink {}",
                    path.display()
                );
                if file_type.is_dir() {
                    stack.push(path);
                    continue;
                }
                ensure!(
                    file_type.is_file(),
                    "completed catalog contains special file {}",
                    path.display()
                );
                let relative = path.strip_prefix(output_dir)?;
                ensure!(
                    expected_roots.iter().any(|root| relative.starts_with(root)),
                    "completed output contains unindexed catalog file {}",
                    relative.display()
                );
            }
        }
    }
    Ok(())
}

fn ensure_safe_terminal_seal_relative_path(relative: &str) -> Result<()> {
    let path = Path::new(relative);
    ensure!(
        !relative.is_empty()
            && !path.is_absolute()
            && path
                .components()
                .all(|component| matches!(component, Component::Normal(_))),
        "operator terminal seal contains unsafe relative path {relative:?}"
    );
    ensure!(
        path.components().all(|component| match component {
            Component::Normal(value) => value.to_str().is_some(),
            _ => false,
        }),
        "operator terminal seal path is not valid UTF-8: {relative:?}"
    );
    Ok(())
}

fn terminal_seal_relative_path(root: &Path, path: &Path) -> Result<String> {
    let relative = path
        .strip_prefix(root)
        .with_context(|| format!("derive terminal-seal path for {}", path.display()))?;
    ensure!(
        !relative.as_os_str().is_empty(),
        "operator terminal seal cannot index its output root"
    );
    let relative = relative
        .to_str()
        .with_context(|| format!("terminal-seal path is not valid UTF-8: {}", path.display()))?
        .replace(std::path::MAIN_SEPARATOR, "/");
    ensure_safe_terminal_seal_relative_path(&relative)?;
    Ok(relative)
}

fn account_terminal_seal_inventory_bytes(
    total: &mut u64,
    relative: &str,
    record_bytes: usize,
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<()> {
    let path_bytes = relative.len();
    let item_bytes = record_bytes
        .checked_add(path_bytes)
        .context("operator terminal seal inventory item byte size overflow")?;
    *total = total
        .checked_add(
            u64::try_from(item_bytes)
                .context("operator terminal seal inventory item bytes do not fit u64")?,
        )
        .context("operator terminal seal inventory byte total overflow")?;
    work_budget.verify_decoded_bytes(*total, OperatorWorkBudgetStage::Finalize)
}

/// Enumerate and stream-hash the exact regular-file set under one output root.
/// Symlinks, special files, unbounded path inventories, and file growth or
/// truncation are rejected before the resulting set can authorize resume.
fn collect_operator_output_seal_files(
    output_dir: &Path,
    excluded_seal_file: &str,
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<Vec<OperatorTerminalSealFile>> {
    ensure!(
        excluded_seal_file == OPERATOR_TERMINAL_SEAL_FILE
            || excluded_seal_file == OPERATOR_DURABLE_OUTPUT_CANDIDATE_SEAL_FILE,
        "unrecognized operator output seal path {excluded_seal_file:?}"
    );
    work_budget.check_deadline(OperatorWorkBudgetStage::Finalize)?;
    let root_metadata = fs::symlink_metadata(output_dir)
        .with_context(|| format!("inspect operator output root {}", output_dir.display()))?;
    ensure!(
        root_metadata.file_type().is_dir() && !root_metadata.file_type().is_symlink(),
        "operator output root must be a real directory: {}",
        output_dir.display()
    );

    let mut directories = Vec::new();
    directories
        .try_reserve_exact(1)
        .context("reserve operator terminal seal directory stack")?;
    directories.push(output_dir.to_path_buf());
    let mut files = Vec::new();
    let mut inventory_bytes = 0_u64;
    let mut total_regular_file_bytes = 0_u64;

    while let Some(directory) = directories.pop() {
        work_budget.check_deadline(OperatorWorkBudgetStage::Finalize)?;
        for entry in fs::read_dir(&directory)
            .with_context(|| format!("traverse operator output {}", directory.display()))?
        {
            work_budget.check_deadline(OperatorWorkBudgetStage::Finalize)?;
            let entry = entry.with_context(|| {
                format!("read operator output entry under {}", directory.display())
            })?;
            let path = entry.path();
            let relative = terminal_seal_relative_path(output_dir, &path)?;
            let file_type = entry
                .file_type()
                .with_context(|| format!("inspect operator output entry {}", path.display()))?;
            ensure!(
                !file_type.is_symlink(),
                "operator output contains symlink {}",
                path.display()
            );

            if relative == excluded_seal_file {
                ensure!(
                    file_type.is_file(),
                    "reserved operator output seal path is not a regular file"
                );
                continue;
            }
            ensure!(
                relative != OPERATOR_TERMINAL_SEAL_FILE
                    && relative != OPERATOR_DURABLE_OUTPUT_CANDIDATE_SEAL_FILE,
                "operator output contains conflicting reserved seal path {relative}"
            );
            if file_type.is_dir() {
                account_terminal_seal_inventory_bytes(
                    &mut inventory_bytes,
                    &relative,
                    size_of::<PathBuf>(),
                    work_budget,
                )?;
                directories
                    .try_reserve(1)
                    .context("grow operator terminal seal directory stack")?;
                directories.push(path);
                continue;
            }
            ensure!(
                file_type.is_file(),
                "operator output contains special file {}",
                path.display()
            );
            account_terminal_seal_inventory_bytes(
                &mut inventory_bytes,
                &relative,
                size_of::<OperatorTerminalSealFile>()
                    .checked_add(64)
                    .context("operator terminal seal hash inventory size overflow")?,
                work_budget,
            )?;
            let (mut file, identity) = open_pinned_regular_file(&path)
                .with_context(|| format!("pin terminal-seal file {}", path.display()))?;
            total_regular_file_bytes = total_regular_file_bytes
                .checked_add(identity.byte_len)
                .context("operator terminal seal regular-file byte total overflow")?;
            work_budget.verify_decoded_bytes(
                total_regular_file_bytes,
                OperatorWorkBudgetStage::Finalize,
            )?;
            let sha256 = sha256_exact_sized_open_file_guarded(
                &mut file,
                &path,
                identity.byte_len,
                work_budget,
                OperatorWorkBudgetStage::Finalize,
            )
            .with_context(|| format!("hash terminal-seal file {}", path.display()))?;
            identity.revalidate_handle(&path, &file)?;
            identity.revalidate_path(&path)?;
            files
                .try_reserve(1)
                .context("grow operator terminal seal file inventory")?;
            files.push(OperatorTerminalSealFile {
                relative_path: relative,
                bytes: identity.byte_len,
                sha256,
            });
        }
    }
    cooperative_stable_sort_by(
        &mut files,
        |left, right| left.relative_path.cmp(&right.relative_path),
        work_budget,
        OperatorWorkBudgetStage::Finalize,
    )?;
    work_budget.check_deadline(OperatorWorkBudgetStage::Finalize)?;
    Ok(files)
}

fn collect_operator_terminal_seal_files(
    output_dir: &Path,
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<Vec<OperatorTerminalSealFile>> {
    collect_operator_output_seal_files(output_dir, OPERATOR_TERMINAL_SEAL_FILE, work_budget)
}

fn collect_durable_output_candidate_seal_files(
    output_dir: &Path,
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<Vec<OperatorTerminalSealFile>> {
    collect_operator_output_seal_files(
        output_dir,
        OPERATOR_DURABLE_OUTPUT_CANDIDATE_SEAL_FILE,
        work_budget,
    )
}

fn preflight_completed_canonical_parquet(
    path: &Path,
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<u64> {
    let stage = OperatorWorkBudgetStage::CatalogProjection;
    let (mut file, identity) = open_pinned_regular_file(path)
        .with_context(|| format!("pin completed canonical Parquet {}", path.display()))?;
    let trailer = verify_parquet_file_trailer_preflight(&mut file, path, work_budget, stage)?;
    ensure!(
        trailer.file_bytes == identity.byte_len,
        "completed canonical Parquet {} changed length before metadata preflight",
        path.display()
    );
    let builder_file = file
        .try_clone()
        .with_context(|| format!("clone completed canonical Parquet {}", path.display()))?;
    let builder = guarded_operation_outcome(work_budget, stage, || {
        ParquetRecordBatchReaderBuilder::try_new(builder_file)
            .with_context(|| format!("read completed canonical metadata {}", path.display()))
    })??;
    let metadata = verify_single_parquet_metadata_budget(builder.metadata(), work_budget, stage)?;
    ensure!(
        metadata.rows > 0 && metadata.row_groups > 0,
        "completed canonical Parquet {} must contain rows and row groups",
        path.display()
    );
    let accounted_bytes = trailer
        .file_bytes
        .checked_add(trailer.footer_metadata_bytes)
        .and_then(|total| total.checked_add(metadata.uncompressed_bytes))
        .context("completed canonical Parquet accounted byte total overflow")?;
    work_budget.verify_decoded_bytes(accounted_bytes, stage)?;
    identity.revalidate_handle(path, &file)?;
    identity.revalidate_path(path)?;
    Ok(metadata.rows)
}

fn canonical_relative_for_catalog_subroot(catalog_subroot: &Path) -> Result<PathBuf> {
    let relative = catalog_subroot
        .strip_prefix(NT_CATALOGS_DIR)
        .with_context(|| {
            format!(
                "completed non-trade catalog subroot must be below {NT_CATALOGS_DIR}: {}",
                catalog_subroot.display()
            )
        })?;
    ensure!(
        !relative.as_os_str().is_empty()
            && relative
                .components()
                .all(|component| matches!(component, Component::Normal(_))),
        "completed non-trade catalog subroot has unsafe relative path {}",
        catalog_subroot.display()
    );
    Ok(relative.join(CANONICAL_TABLE_FILE))
}

fn insert_expected_catalog_files(
    expected_files: &mut BTreeSet<PathBuf>,
    catalog_relative: &Path,
    inventory: &CatalogInventorySummary,
) -> Result<()> {
    for relative in inventory.files.keys() {
        let path = catalog_relative.join(relative);
        ensure!(
            expected_files.insert(path.clone()),
            "duplicate completed catalog file {}",
            path.display()
        );
    }
    Ok(())
}

struct CompletedOperatorOutputVerification<'a> {
    spec: &'a RunSpec,
    expected_source_proof: &'a SourceProofReport,
    accepted: &'a AcceptedDataset,
    fingerprint: &'a ConversionFingerprint,
    output_dir: &'a Path,
    seal: &'a OperatorTerminalSeal,
    current_files: &'a [OperatorTerminalSealFile],
    verify_physical_catalog_view: bool,
    work_budget: &'a OperatorWorkBudgetGuard,
}

fn verify_completed_operator_output_against_seal(
    verification: CompletedOperatorOutputVerification<'_>,
) -> Result<OperatorRunSummary> {
    let CompletedOperatorOutputVerification {
        spec,
        expected_source_proof,
        accepted,
        fingerprint,
        output_dir,
        seal,
        current_files,
        verify_physical_catalog_view,
        work_budget,
    } = verification;
    work_budget.check_deadline(OperatorWorkBudgetStage::CatalogProjection)?;
    seal.validate_for(spec, fingerprint)?;
    ensure!(
        seal.files.as_slice() == current_files,
        "operator terminal seal exact file set or content hash mismatch"
    );
    let mut expected_files = BTreeSet::from([
        PathBuf::from(ACCEPTED_SOURCE_PROOF_FILE),
        PathBuf::from(RESULT_CONTRACT_FILE),
        PathBuf::from(BACKTEST_RUN_MANIFEST_FILE),
        PathBuf::from(crate::conversion_boundary::CONVERSION_MANIFEST_FILE),
        PathBuf::from(CONVERSION_CHECKPOINT_FILE),
        PathBuf::from(CATALOG_METADATA_FILE),
    ]);
    let checkpoint_path = output_dir.join(CONVERSION_CHECKPOINT_FILE);
    let manifest_path = output_dir.join(crate::conversion_boundary::CONVERSION_MANIFEST_FILE);
    let metadata_path = output_dir.join(CATALOG_METADATA_FILE);
    let checkpoint: ConversionCheckpoint = read_json_artifact_guarded(
        &checkpoint_path,
        work_budget,
        OperatorWorkBudgetStage::CatalogProjection,
    )?;
    checkpoint.validate_for(fingerprint)?;
    ensure!(
        checkpoint.stage == ConversionCheckpointStage::Completed,
        "completed-output verifier requires a completed checkpoint"
    );
    let checkpoint_hash = checkpoint.content_hash()?;
    let manifest: ConversionManifest = read_json_artifact_guarded(
        &manifest_path,
        work_budget,
        OperatorWorkBudgetStage::CatalogProjection,
    )?;
    manifest.validate_for(fingerprint, &checkpoint_hash)?;
    let manifest_hash = manifest.content_hash()?;
    let metadata: ConversionCatalogMetadata = read_json_artifact_guarded(
        &metadata_path,
        work_budget,
        OperatorWorkBudgetStage::CatalogProjection,
    )?;
    metadata.validate_against(&manifest, &manifest_hash, &checkpoint_hash)?;
    ensure!(
        is_lowercase_sha256_hex(&manifest.catalog_hash),
        "completed conversion catalog hash is not lowercase SHA-256"
    );

    let adapter =
        require_registered_source_adapter(&spec.converter.identity, &spec.converter.version)?;
    let index_path = output_dir.join(CONVERSION_TABLES_FILE);
    let mut expected_roots = BTreeSet::new();
    let (canonical_rows, nt_catalog_rows, runtime_manifest) = if adapter.kind
        == SourceAdapterKind::CsvNativeTrades
    {
        ensure!(
            spec.manifest.catalog_inputs.len() == 1,
            "trade completion requires exactly one RunSpec catalog input"
        );
        ensure!(
            !index_path.exists(),
            "trade completion must not contain {CONVERSION_TABLES_FILE}"
        );
        ensure!(
            manifest.nt_data_type == NT_DATA_TYPE_TRADE_TICK,
            "trade completion NT data type mismatch"
        );
        let relative = completed_catalog_relative_path(spec, &manifest.output_catalog_uri)?;
        ensure!(
            relative == Path::new(CATALOG_DIR),
            "trade completion catalog root must be {CATALOG_DIR}"
        );
        expected_roots.insert(relative.clone());
        let inventory = preflight_completed_catalog(
            &output_dir.join(&relative),
            &manifest.nt_data_type,
            work_budget,
        )?;
        insert_expected_catalog_files(&mut expected_files, &relative, &inventory)?;
        let actual_hash = logical_catalog_hash_guarded(&output_dir.join(&relative), work_budget)?;
        ensure!(
            actual_hash == manifest.catalog_hash,
            "completed trade catalog hash mismatch"
        );
        let post_inventory = preflight_completed_catalog(
            &output_dir.join(&relative),
            &manifest.nt_data_type,
            work_budget,
        )?;
        ensure!(
            post_inventory == inventory,
            "completed trade catalog changed during verification"
        );
        let canonical_rows = u64::try_from(manifest.canonical_rows)
            .context("completed trade canonical rows do not fit u64")?;
        let canonical_relative = PathBuf::from(CANONICAL_ARTIFACT_FILE);
        let canonical_parquet_rows = preflight_completed_canonical_parquet(
            &output_dir.join(&canonical_relative),
            work_budget,
        )?;
        ensure!(
            inventory.data_rows == canonical_rows && canonical_parquet_rows == canonical_rows,
            "completed trade canonical/catalog rows do not match manifest"
        );
        ensure!(
            expected_files.insert(canonical_relative),
            "duplicate completed trade canonical artifact"
        );
        (
            canonical_rows,
            inventory.data_rows,
            local_run_manifest_for_output(spec, output_dir)?,
        )
    } else if index_path.exists() {
        ensure!(
            expected_files.insert(PathBuf::from(CONVERSION_TABLES_FILE)),
            "duplicate conversion tables artifact"
        );
        let indexed: Vec<ConversionTableRecord> = read_json_artifact_guarded(
            &index_path,
            work_budget,
            OperatorWorkBudgetStage::CatalogProjection,
        )?;
        ensure!(
            indexed.len() > 1,
            "multi-table completion index must contain more than one table"
        );
        ensure!(
            indexed.len() == spec.manifest.catalog_inputs.len(),
            "multi-table completion index has {} records but the current RunSpec binds {} catalog inputs",
            indexed.len(),
            spec.manifest.catalog_inputs.len()
        );
        let mut canonical_rows = 0_u64;
        let mut nt_catalog_rows = 0_u64;
        let mut inventories = BTreeMap::new();
        let mut identities = BTreeSet::new();
        let mut rows_by_data_type: BTreeMap<String, usize> = BTreeMap::new();
        let primary_relative = completed_catalog_relative_path(spec, &manifest.output_catalog_uri)?;
        let mut primary_matches = 0_usize;
        for record in &indexed {
            record.validate()?;
            ensure!(
                identities.insert((
                    record.table_family.clone(),
                    record.nt_instrument_id.clone(),
                    record.data_type.clone(),
                    record.bar_spec.clone(),
                )),
                "completed conversion tables index contains duplicate table identity {}/{}/{}",
                record.table_family,
                record.nt_instrument_id,
                record.data_type
            );
            let rows_for_type = rows_by_data_type
                .entry(record.data_type.clone())
                .or_insert(0);
            *rows_for_type = rows_for_type
                .checked_add(record.rows)
                .context("completed conversion tables row total overflow")?;
            let relative = PathBuf::from(&record.subroot_uri);
            ensure!(
                relative.starts_with(NT_CATALOGS_DIR),
                "multi-table catalog subroot must be under {NT_CATALOGS_DIR}"
            );
            ensure!(
                expected_roots.insert(relative.clone()),
                "duplicate completed catalog subroot {}",
                relative.display()
            );
            let subroot = output_dir.join(&relative);
            let inventory = preflight_completed_catalog(&subroot, &record.data_type, work_budget)?;
            insert_expected_catalog_files(&mut expected_files, &relative, &inventory)?;
            let record_rows =
                u64::try_from(record.rows).context("conversion table rows do not fit u64")?;
            let canonical_relative = canonical_relative_for_catalog_subroot(&relative)?;
            let canonical_parquet_rows = preflight_completed_canonical_parquet(
                &output_dir.join(&canonical_relative),
                work_budget,
            )?;
            ensure!(
                inventory.data_rows == record_rows && canonical_parquet_rows == record_rows,
                "completed canonical/catalog {} rows do not match index",
                relative.display()
            );
            ensure!(
                expected_files.insert(canonical_relative.clone()),
                "duplicate completed canonical file {}",
                canonical_relative.display()
            );
            let actual_hash = logical_catalog_hash_guarded(&subroot, work_budget)
                .with_context(|| format!("recompute guarded catalog hash {}", subroot.display()))?;
            ensure!(
                actual_hash == record.catalog_hash,
                "completed conversion tables index subroot {} hash mismatch: recorded {:?}, recomputed {:?}",
                record.subroot_uri,
                record.catalog_hash,
                actual_hash
            );
            if record.nt_instrument_id == manifest.nt_instrument_id
                && record.data_type == manifest.nt_data_type
                && record.catalog_hash == manifest.catalog_hash
            {
                ensure!(
                    relative == primary_relative,
                    "completed primary catalog subroot {} does not match manifest URI {}",
                    relative.display(),
                    primary_relative.display()
                );
                primary_matches = primary_matches
                    .checked_add(1)
                    .context("completed primary match count overflow")?;
            }
            canonical_rows = canonical_rows
                .checked_add(record_rows)
                .context("completed canonical row total overflow")?;
            nt_catalog_rows = nt_catalog_rows
                .checked_add(inventory.data_rows)
                .context("completed NT row total overflow")?;
            inventories.insert(relative.clone(), inventory);
        }
        ensure!(
            rows_by_data_type == manifest.effective_catalog_rows_by_nt_data_type(),
            "completed conversion tables per-data-type rows {:?} do not match manifest {:?}",
            rows_by_data_type,
            manifest.effective_catalog_rows_by_nt_data_type()
        );
        ensure!(
            primary_matches == 1,
            "completed conversion tables index must contain exactly one guarded primary record matching the manifest, found {primary_matches}"
        );
        verify_catalog_root_set(output_dir, &expected_roots, work_budget)?;
        for record in &indexed {
            let relative = PathBuf::from(&record.subroot_uri);
            let post_inventory = preflight_completed_catalog(
                &output_dir.join(&relative),
                &record.data_type,
                work_budget,
            )?;
            ensure!(
                inventories.get(&relative) == Some(&post_inventory),
                "completed catalog {} changed during verification",
                relative.display()
            );
        }
        (
            canonical_rows,
            nt_catalog_rows,
            bind_completed_catalog_inputs(spec, output_dir, &indexed)?,
        )
    } else {
        ensure!(
            spec.manifest.catalog_inputs.len() == 1,
            "completed RunSpec binds {} catalog inputs but is missing {CONVERSION_TABLES_FILE}",
            spec.manifest.catalog_inputs.len()
        );
        let relative = completed_catalog_relative_path(spec, &manifest.output_catalog_uri)?;
        ensure!(
            relative.starts_with(NT_CATALOGS_DIR),
            "non-trade catalog root must be under {NT_CATALOGS_DIR}"
        );
        expected_roots.insert(relative.clone());
        let inventory = preflight_completed_catalog(
            &output_dir.join(&relative),
            &manifest.nt_data_type,
            work_budget,
        )?;
        insert_expected_catalog_files(&mut expected_files, &relative, &inventory)?;
        let actual_hash = logical_catalog_hash_guarded(&output_dir.join(&relative), work_budget)?;
        ensure!(
            actual_hash == manifest.catalog_hash,
            "completed non-trade catalog hash mismatch"
        );
        let post_inventory = preflight_completed_catalog(
            &output_dir.join(&relative),
            &manifest.nt_data_type,
            work_budget,
        )?;
        ensure!(
            post_inventory == inventory,
            "completed non-trade catalog changed during verification"
        );
        let canonical_rows = u64::try_from(manifest.canonical_rows)
            .context("completed non-trade canonical rows do not fit u64")?;
        let canonical_relative = canonical_relative_for_catalog_subroot(&relative)?;
        let canonical_parquet_rows = preflight_completed_canonical_parquet(
            &output_dir.join(&canonical_relative),
            work_budget,
        )?;
        ensure!(
            inventory.data_rows == canonical_rows && canonical_parquet_rows == canonical_rows,
            "completed non-trade canonical/catalog rows do not match manifest"
        );
        ensure!(
            expected_files.insert(canonical_relative.clone()),
            "duplicate completed canonical file {}",
            canonical_relative.display()
        );
        let mut runtime_manifest = spec.manifest.clone();
        let runtime_input = runtime_manifest
            .catalog_inputs
            .first_mut()
            .context("completed non-trade manifest has no catalog input")?;
        runtime_input.catalog_path = output_dir
            .join(&relative)
            .to_str()
            .context("completed non-trade catalog path is not UTF-8")?
            .to_string();
        runtime_input.catalog_fs_protocol = CATALOG_FS_PROTOCOL_NONE.to_string();
        runtime_input.catalog_fs_storage_options.clear();
        runtime_input.catalog_fs_rust_storage_options.clear();
        (canonical_rows, inventory.data_rows, runtime_manifest)
    };
    verify_catalog_root_set(output_dir, &expected_roots, work_budget)?;
    ensure!(
        expected_files.insert(PathBuf::from(CATALOG_RUN_VIEW_AUTHORITY_FILE)),
        "duplicate catalog run-view authority artifact"
    );
    if verify_physical_catalog_view {
        load_and_verify_catalog_run_view_authority_guarded(
            spec,
            &runtime_manifest,
            output_dir,
            work_budget,
        )?;
    } else {
        load_catalog_run_view_authority_guarded(spec, &runtime_manifest, output_dir, work_budget)?;
    }

    let written_source_proof: SourceProofReport = read_json_artifact_guarded(
        &output_dir.join(ACCEPTED_SOURCE_PROOF_FILE),
        work_budget,
        OperatorWorkBudgetStage::Finalize,
    )?;
    ensure!(
        written_source_proof == *expected_source_proof,
        "sealed accepted-source proof does not match the proof accepted from current controls"
    );

    let written_run_manifest: BacktestRunManifestArtifact = read_json_artifact_guarded(
        &output_dir.join(BACKTEST_RUN_MANIFEST_FILE),
        work_budget,
        OperatorWorkBudgetStage::Finalize,
    )?;
    let expected_run_manifest = spec
        .manifest
        .to_artifact_manifest()
        .map_err(|error| anyhow::anyhow!("build expected run-manifest artifact: {error}"))?;
    ensure!(
        written_run_manifest.manifest_version == BACKTEST_RUN_MANIFEST_ARTIFACT_VERSION
            && written_run_manifest == expected_run_manifest,
        "sealed backtest run manifest does not match the current RunSpec"
    );

    let contract: BacktestResultContract = read_json_artifact_guarded(
        &output_dir.join(RESULT_CONTRACT_FILE),
        work_budget,
        OperatorWorkBudgetStage::Finalize,
    )?;
    contract
        .validate()
        .map_err(|error| anyhow::anyhow!("sealed result contract validation failed: {error}"))?;
    let metadata_hash = metadata.content_hash()?;
    ensure!(
        contract.run_id == spec.manifest.run_id
            && contract.manifest_hash == spec.manifest.manifest_hash()
            && contract.source_proof_id == accepted.source_proof_id
            && contract.source_proof_version == accepted.source_proof_version
            && contract.acceptance_mode == accepted.acceptance_mode
            && contract.accepted_by == accepted.accepted_by
            && contract.accepted_at == accepted.accepted_at
            && contract.accepted_object_sha256 == accepted.accepted_object_sha256
            && contract.converter_identity == fingerprint.converter_identity
            && contract.converter_version == fingerprint.converter_version
            && contract.converter_config_hash == fingerprint.converter_config_hash
            && contract.conversion_manifest_hash == manifest_hash
            && contract.conversion_checkpoint_hash == checkpoint_hash
            && contract.catalog_hash == manifest.catalog_hash
            && contract.catalog_metadata_hash == metadata_hash
            && contract.strategy_config_hash == spec.manifest.strategy_config_hash
            && contract.execution_model == spec.manifest.execution_model,
        "sealed result contract does not match current source, conversion, catalog, or RunSpec identities"
    );
    let expected_catalog_data_types = spec
        .manifest
        .catalog_inputs
        .iter()
        .map(|input| input.data_type.clone())
        .collect::<Vec<_>>();
    ensure!(
        contract.catalog_data_types == expected_catalog_data_types,
        "sealed result contract catalog_data_types do not match the current RunSpec"
    );
    ensure!(
        contract.artifact_uris.source_proof_uri
            == portable_artifact_uri(&spec.manifest.output_prefix, ACCEPTED_SOURCE_PROOF_FILE)
            && contract.artifact_uris.catalog_metadata_uri
                == portable_artifact_uri(&spec.manifest.output_prefix, CATALOG_METADATA_FILE)
            && contract.artifact_uris.result_contract_uri
                == portable_artifact_uri(&spec.manifest.output_prefix, RESULT_CONTRACT_FILE)
            && contract.artifact_uris.nt_catalog_uri == manifest.output_catalog_uri,
        "sealed result contract artifact URIs do not match current output identities"
    );
    let canonical_contract_relative =
        completed_catalog_relative_path(spec, &contract.artifact_uris.canonical_table_uri)?;
    ensure!(
        expected_files.contains(&canonical_contract_relative),
        "sealed result contract canonical_table_uri does not identify a sealed canonical Parquet"
    );

    let sealed_paths = seal
        .files
        .iter()
        .map(|file| PathBuf::from(&file.relative_path))
        .collect::<BTreeSet<_>>();
    let published_proof_present = sealed_paths.contains(Path::new(PUBLISHED_CATALOG_PROOF_FILE));
    ensure!(
        published_proof_present == metadata.catalog_consumption_proven(),
        "sealed published-catalog proof presence must exactly match catalog metadata: proof_present={}, metadata_proven={}",
        published_proof_present,
        metadata.catalog_consumption_proven()
    );
    if metadata.catalog_consumption_proven() {
        let proof: PublishedCatalogProof = read_json_artifact_guarded(
            &output_dir.join(PUBLISHED_CATALOG_PROOF_FILE),
            work_budget,
            OperatorWorkBudgetStage::Finalize,
        )?;
        proof.validate_against(&metadata, &contract, spec)?;
        expected_files.insert(PathBuf::from(PUBLISHED_CATALOG_PROOF_FILE));
    }
    ensure!(
        expected_files == sealed_paths,
        "operator terminal seal file set contains missing or unexpected output artifacts: expected {expected_files:?}, sealed {sealed_paths:?}"
    );

    work_budget.verify_source_rows(canonical_rows, OperatorWorkBudgetStage::CatalogProjection)?;
    let summary = OperatorRunSummary {
        canonical_rows,
        nt_catalog_rows,
        catalog_hash: manifest.catalog_hash,
    };
    ensure!(
        seal.summary() == summary,
        "operator terminal seal summary does not match verified canonical/catalog output"
    );
    Ok(summary)
}

fn commit_operator_terminal_seal(
    spec: &RunSpec,
    expected_source_proof: &SourceProofReport,
    accepted: &AcceptedDataset,
    fingerprint: &ConversionFingerprint,
    output_dir: &Path,
    expected_summary: &OperatorRunSummary,
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<()> {
    let seal_path = output_dir.join(OPERATOR_TERMINAL_SEAL_FILE);
    let files = collect_operator_terminal_seal_files(output_dir, work_budget)?;
    let seal = OperatorTerminalSeal::new(spec, fingerprint.clone(), expected_summary, files);
    let verified =
        verify_completed_operator_output_against_seal(CompletedOperatorOutputVerification {
            spec,
            expected_source_proof,
            accepted,
            fingerprint,
            output_dir,
            seal: &seal,
            current_files: &seal.files,
            verify_physical_catalog_view: true,
            work_budget,
        })?;
    ensure!(
        verified == *expected_summary,
        "operator terminal seal candidate summary changed during precommit verification"
    );
    let post_verification_files = collect_operator_terminal_seal_files(output_dir, work_budget)?;
    ensure!(
        post_verification_files.as_slice() == seal.files.as_slice(),
        "operator output changed after terminal-seal verification"
    );
    let seal_bytes = crate::reference_artifact::canonical_json_bytes(&seal)
        .context("serialize operator terminal seal")?;
    work_budget.verify_decoded_bytes(
        u64::try_from(seal_bytes.len()).context("operator terminal seal bytes do not fit u64")?,
        OperatorWorkBudgetStage::Finalize,
    )?;

    // This create-only rename is the final fallible operation. No artifact is
    // accepted as complete before it, and no later I/O can reclassify it.
    persist_immutable_local_bytes_guarded(
        &seal_path,
        &seal_bytes,
        "operator terminal seal",
        work_budget,
        OperatorWorkBudgetStage::Finalize,
    )
}

/// Seal the exact final local byte set immediately before a durable run
/// attempts its sole remote terminal create. This candidate is local
/// integrity evidence only: neither this function nor its artifact can mint a
/// [`DurableCompletionLocator`].
fn commit_durable_operator_output_candidate(
    spec: &RunSpec,
    fingerprint: &ConversionFingerprint,
    output_dir: &Path,
    expected_summary: &OperatorRunSummary,
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<()> {
    let candidate_path = output_dir.join(OPERATOR_DURABLE_OUTPUT_CANDIDATE_SEAL_FILE);
    let files = collect_durable_output_candidate_seal_files(output_dir, work_budget)?;
    let candidate =
        OperatorDurableOutputCandidateSeal::new(spec, fingerprint.clone(), expected_summary, files);
    candidate.validate_for(spec, fingerprint)?;
    ensure!(
        candidate.summary() == *expected_summary,
        "durable output candidate summary changed during precommit validation"
    );
    let post_validation_files =
        collect_durable_output_candidate_seal_files(output_dir, work_budget)?;
    ensure!(
        post_validation_files.as_slice() == candidate.files.as_slice(),
        "operator output changed after durable candidate validation"
    );
    let candidate_bytes =
        serialize_json_to_vec_guarded(&candidate, work_budget, OperatorWorkBudgetStage::Finalize)
            .context("serialize canonical durable output candidate seal")?;
    work_budget.verify_decoded_bytes(
        u64::try_from(candidate_bytes.len())
            .context("durable output candidate seal bytes do not fit u64")?,
        OperatorWorkBudgetStage::Finalize,
    )?;
    persist_immutable_local_bytes_guarded(
        &candidate_path,
        &candidate_bytes,
        "durable output candidate seal",
        work_budget,
        OperatorWorkBudgetStage::Finalize,
    )
}

/// Make the in-process test runner model production's durable local boundary
/// without introducing a second production execution path.
#[cfg(test)]
pub(crate) fn convert_test_terminal_output_to_durable_candidate(
    spec: &RunSpec,
    output_dir: &Path,
    registry: &VerifiedSourceBindingRegistry,
    expected_summary: &OperatorRunSummary,
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<()> {
    let observed = verify_completed_operator_output(spec, output_dir, registry, work_budget)?;
    ensure!(
        observed == *expected_summary,
        "test runner terminal summary changed before durable candidate conversion"
    );
    fs::remove_file(output_dir.join(OPERATOR_TERMINAL_SEAL_FILE))
        .context("remove test-only local terminal seal before candidate conversion")?;
    let fingerprint = validated_operator_output_seal_fingerprint(spec, registry)?;
    commit_durable_operator_output_candidate(
        spec,
        &fingerprint,
        output_dir,
        expected_summary,
        work_budget,
    )
}

/// Verify a committed operator output against current frozen controls and
/// derive its report summary exclusively from sealed completion/catalog bytes.
#[cfg(test)]
pub(crate) fn verify_completed_operator_output(
    spec: &RunSpec,
    output_dir: &Path,
    registry: &VerifiedSourceBindingRegistry,
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<OperatorRunSummary> {
    registry.reassert_for(spec)?;
    validate_converter_config(&spec.converter)?;
    let (expected_source_proof, accepted) = accepted_dataset_for_run_spec_hash_with_registry(
        spec,
        &spec.accepted_object.sha256,
        registry.registry(),
    )?;
    validate_converter_table_family(&spec.converter, &accepted.table_family)?;
    let seal: OperatorTerminalSeal = read_json_artifact_guarded(
        &output_dir.join(OPERATOR_TERMINAL_SEAL_FILE),
        work_budget,
        OperatorWorkBudgetStage::Finalize,
    )?;
    let current_files = collect_operator_terminal_seal_files(output_dir, work_budget)?;
    let fingerprint = conversion_fingerprint_for(spec, &accepted, registry)?;
    verify_completed_operator_output_against_seal(CompletedOperatorOutputVerification {
        spec,
        expected_source_proof: &expected_source_proof,
        accepted: &accepted,
        fingerprint: &fingerprint,
        output_dir,
        seal: &seal,
        current_files: &current_files,
        verify_physical_catalog_view: true,
        work_budget,
    })
}

/// Deadline-free observation of pre-terminal durable local evidence. A
/// `Candidate` variant never means the remote completion manifest exists.
pub(crate) enum DurableOutputCandidateSealProbe {
    Absent,
    Candidate(OperatorRunSummary),
}

fn read_canonical_operator_output_seal_capped<T>(
    seal_path: &Path,
    max_seal_bytes: u64,
    role: &str,
) -> Result<Option<T>>
where
    T: DeserializeOwned + Serialize,
{
    ensure!(max_seal_bytes > 0, "{role} byte cap must be positive");
    match fs::symlink_metadata(seal_path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| format!("inspect {role} {}", seal_path.display()));
        }
        Ok(metadata) => ensure!(
            metadata.file_type().is_file() && !metadata.file_type().is_symlink(),
            "occupied {role} path is not a regular file"
        ),
    }
    let (mut file, identity) = open_pinned_regular_file(seal_path)?;
    ensure!(
        identity.byte_len > 0 && identity.byte_len <= max_seal_bytes,
        "{role} byte length {} exceeds explicit cap {max_seal_bytes}",
        identity.byte_len
    );
    let capacity = usize::try_from(identity.byte_len)
        .with_context(|| format!("{role} byte length does not fit usize"))?;
    let bounded_capacity = capacity
        .checked_add(1)
        .with_context(|| format!("{role} capped read length overflow"))?;
    let bounded_read_len = identity
        .byte_len
        .checked_add(1)
        .with_context(|| format!("{role} capped read length overflow"))?;
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(bounded_capacity)
        .with_context(|| format!("reserve capped {role} payload"))?;
    (&mut file)
        .take(bounded_read_len)
        .read_to_end(&mut bytes)
        .with_context(|| format!("read pinned {role} {}", seal_path.display()))?;
    ensure!(
        bytes.len() == capacity,
        "{role} changed length while reading"
    );
    identity.revalidate(seal_path, &file)?;
    let seal: T = serde_json::from_slice(&bytes)
        .with_context(|| format!("parse {role} {}", seal_path.display()))?;
    let canonical = serde_json::to_vec(&seal)
        .map_err(anyhow::Error::from)
        .with_context(|| format!("serialize canonical {role}"))?;
    ensure!(canonical == bytes, "{role} bytes are not canonical");
    Ok(Some(seal))
}

fn validated_operator_output_seal_fingerprint(
    spec: &RunSpec,
    registry: &VerifiedSourceBindingRegistry,
) -> Result<ConversionFingerprint> {
    registry.reassert_for(spec)?;
    validate_converter_config(&spec.converter)?;
    let (_expected_source_proof, accepted) = accepted_dataset_for_run_spec_hash_with_registry(
        spec,
        &spec.accepted_object.sha256,
        registry.registry(),
    )?;
    validate_converter_table_family(&spec.converter, &accepted.table_family)?;
    conversion_fingerprint_for(spec, &accepted, registry)
}

pub(crate) fn probe_durable_output_candidate_seal_summary_capped(
    spec: &RunSpec,
    output_dir: &Path,
    registry: &VerifiedSourceBindingRegistry,
    max_seal_bytes: u64,
) -> Result<DurableOutputCandidateSealProbe> {
    let candidate_path = output_dir.join(OPERATOR_DURABLE_OUTPUT_CANDIDATE_SEAL_FILE);
    let Some(candidate): Option<OperatorDurableOutputCandidateSeal> =
        read_canonical_operator_output_seal_capped(
            &candidate_path,
            max_seal_bytes,
            "durable output candidate seal",
        )?
    else {
        return Ok(DurableOutputCandidateSealProbe::Absent);
    };
    let fingerprint = validated_operator_output_seal_fingerprint(spec, registry)?;
    candidate.validate_for(spec, &fingerprint)?;
    Ok(DurableOutputCandidateSealProbe::Candidate(
        candidate.summary(),
    ))
}

fn preflight_completed_output_before_inspection(
    spec: &RunSpec,
    expected_source_proof: &SourceProofReport,
    accepted: &AcceptedDataset,
    output_dir: &Path,
    fingerprint: &ConversionFingerprint,
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<bool> {
    let checkpoint_path = output_dir.join(CONVERSION_CHECKPOINT_FILE);
    match fs::symlink_metadata(&checkpoint_path) {
        Ok(metadata) => ensure!(
            metadata.file_type().is_file(),
            "conversion checkpoint {} is not a regular file",
            checkpoint_path.display()
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("inspect checkpoint path {}", checkpoint_path.display()));
        }
    }
    let checkpoint: ConversionCheckpoint = read_json_artifact_guarded(
        &checkpoint_path,
        work_budget,
        OperatorWorkBudgetStage::CatalogProjection,
    )?;
    checkpoint.validate_for(fingerprint)?;
    ensure!(
        checkpoint.stage == ConversionCheckpointStage::Completed,
        "legacy nonterminal conversion checkpoint cannot be overwritten by the immutable completion protocol"
    );
    let seal_path = output_dir.join(OPERATOR_TERMINAL_SEAL_FILE);
    match fs::symlink_metadata(&seal_path) {
        Ok(metadata) => ensure!(
            metadata.file_type().is_file(),
            "operator terminal seal {} is not a regular file",
            seal_path.display()
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("inspect terminal seal path {}", seal_path.display()));
        }
    }
    let seal: OperatorTerminalSeal =
        read_json_artifact_guarded(&seal_path, work_budget, OperatorWorkBudgetStage::Finalize)?;
    let current_files = collect_operator_terminal_seal_files(output_dir, work_budget)?;
    verify_completed_operator_output_against_seal(CompletedOperatorOutputVerification {
        spec,
        expected_source_proof,
        accepted,
        fingerprint,
        output_dir,
        seal: &seal,
        current_files: &current_files,
        verify_physical_catalog_view: false,
        work_budget,
    })?;
    Ok(true)
}

#[derive(Debug)]
struct TransientCatalogRootLease {
    catalog_root: PathBuf,
    catalog_root_handle: fs::File,
    #[cfg(unix)]
    catalog_root_device: u64,
    #[cfg(unix)]
    catalog_root_inode: u64,
    #[cfg(unix)]
    catalog_root_uid: u32,
    #[cfg(unix)]
    catalog_root_mode: u32,
}

#[cfg(unix)]
const PRIVATE_CATALOG_ROOT_MODE: u32 = 0o700;
#[cfg(unix)]
const UNIX_PERMISSION_MASK: u32 = 0o777;

impl TransientCatalogRootLease {
    fn acquire(output_dir: &Path) -> Result<Self> {
        let catalog_root =
            crate::atomic_artifact_write::unique_temp_path(&output_dir.join(CATALOG_DIR))
                .context("derive unique transient catalog root")?;
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        builder.mode(PRIVATE_CATALOG_ROOT_MODE);
        builder.create(&catalog_root).with_context(|| {
            format!(
                "atomically claim unique transient catalog root {}",
                catalog_root.display()
            )
        })?;
        #[cfg(unix)]
        fs::set_permissions(
            &catalog_root,
            fs::Permissions::from_mode(PRIVATE_CATALOG_ROOT_MODE),
        )
        .with_context(|| {
            format!(
                "restrict transient catalog root permissions {}",
                catalog_root.display()
            )
        })?;
        Self::open_claimed(catalog_root)
    }

    fn acquire_stable(output_dir: &Path) -> Result<Self> {
        let catalog_root = output_dir.join(CATALOG_DIR);
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        builder.mode(PRIVATE_CATALOG_ROOT_MODE);
        match builder.create(&catalog_root) {
            Ok(()) => {
                #[cfg(unix)]
                fs::set_permissions(
                    &catalog_root,
                    fs::Permissions::from_mode(PRIVATE_CATALOG_ROOT_MODE),
                )
                .with_context(|| {
                    format!(
                        "restrict stable transient catalog root permissions {}",
                        catalog_root.display()
                    )
                })?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                let metadata = fs::symlink_metadata(&catalog_root).with_context(|| {
                    format!(
                        "inspect stable transient catalog root {}",
                        catalog_root.display()
                    )
                })?;
                ensure!(
                    metadata.file_type().is_dir(),
                    "stable transient catalog root {} is not a real directory",
                    catalog_root.display()
                );
                #[cfg(unix)]
                {
                    let output_metadata = fs::metadata(output_dir).with_context(|| {
                        format!("inspect stable catalog parent {}", output_dir.display())
                    })?;
                    ensure!(
                        metadata.uid() == output_metadata.uid(),
                        "stable transient catalog root {} is owned by a different user",
                        catalog_root.display()
                    );
                    ensure!(
                        metadata.permissions().mode() & UNIX_PERMISSION_MASK
                            == PRIVATE_CATALOG_ROOT_MODE,
                        "stable transient catalog root {} must retain private {:o} permissions",
                        catalog_root.display(),
                        PRIVATE_CATALOG_ROOT_MODE
                    );
                }
            }
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "claim stable transient catalog root {}",
                        catalog_root.display()
                    )
                });
            }
        }
        Self::open_claimed(catalog_root)
    }

    fn open_claimed(catalog_root: PathBuf) -> Result<Self> {
        let catalog_root_handle = match fs::File::open(&catalog_root) {
            Ok(handle) => handle,
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "open claimed unique transient catalog root {}; retained it because pathname cleanup is prohibited",
                        catalog_root.display()
                    )
                });
            }
        };
        let catalog_root = catalog_root.canonicalize().with_context(|| {
            format!(
                "canonicalize transient catalog root {}",
                catalog_root.display()
            )
        })?;
        let catalog_root_metadata = catalog_root_handle
            .metadata()
            .with_context(|| format!("stat transient catalog root {}", catalog_root.display()))?;
        ensure!(
            catalog_root_metadata.is_dir(),
            "transient catalog root {} is not a directory",
            catalog_root.display()
        );
        let lease = Self {
            catalog_root,
            catalog_root_handle,
            #[cfg(unix)]
            catalog_root_device: catalog_root_metadata.dev(),
            #[cfg(unix)]
            catalog_root_inode: catalog_root_metadata.ino(),
            #[cfg(unix)]
            catalog_root_uid: catalog_root_metadata.uid(),
            #[cfg(unix)]
            catalog_root_mode: catalog_root_metadata.permissions().mode() & UNIX_PERMISSION_MASK,
        };
        lease.revalidate()?;
        Ok(lease)
    }

    fn revalidate(&self) -> Result<()> {
        let catalog_root_now = self.catalog_root.canonicalize().with_context(|| {
            format!(
                "canonicalize leased transient catalog root {}",
                self.catalog_root.display()
            )
        })?;
        ensure!(
            catalog_root_now == self.catalog_root,
            "transient catalog root canonical identity changed"
        );
        let catalog_path_metadata = fs::metadata(&self.catalog_root).with_context(|| {
            format!(
                "stat leased transient catalog root path {}",
                self.catalog_root.display()
            )
        })?;
        let catalog_handle_metadata = self.catalog_root_handle.metadata().with_context(|| {
            format!(
                "stat held transient catalog root {}",
                self.catalog_root.display()
            )
        })?;
        ensure!(
            catalog_path_metadata.is_dir() && catalog_handle_metadata.is_dir(),
            "transient catalog root {} is no longer a directory",
            self.catalog_root.display()
        );
        #[cfg(unix)]
        ensure!(
            catalog_path_metadata.dev() == self.catalog_root_device
                && catalog_path_metadata.ino() == self.catalog_root_inode
                && catalog_handle_metadata.dev() == self.catalog_root_device
                && catalog_handle_metadata.ino() == self.catalog_root_inode
                && catalog_path_metadata.uid() == self.catalog_root_uid
                && catalog_handle_metadata.uid() == self.catalog_root_uid
                && catalog_path_metadata.permissions().mode() & UNIX_PERMISSION_MASK
                    == self.catalog_root_mode
                && catalog_handle_metadata.permissions().mode() & UNIX_PERMISSION_MASK
                    == self.catalog_root_mode
                && self.catalog_root_mode == PRIVATE_CATALOG_ROOT_MODE,
            "transient catalog root identity, owner, or private mode changed"
        );
        Ok(())
    }

    fn finish_retained(self, work_budget: &OperatorWorkBudgetGuard) -> Result<()> {
        work_budget.check_deadline(OperatorWorkBudgetStage::Publish)?;
        self.revalidate()?;
        // The root is deliberately retained. A path-based recursive cleanup
        // could race with a root or child replacement and delete foreign data.
        // Stable projection roots are exact-reconciled retry state; unique
        // hydration roots share the lifecycle of their owning output attempt.
        work_budget.check_deadline(OperatorWorkBudgetStage::Publish)
    }
}

/// Evidence that NautilusTrader consumed the published catalog.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublishedCatalogProof {
    pub proof_version: String,
    pub catalog_uri: String,
    pub catalog_fs_protocol: String,
    pub publication_receipt_uri: String,
    pub publication_receipt_sha256: String,
    pub publication_receipt_version_id: String,
    pub publication_physical_manifest_sha256: String,
    pub expected_iterations: usize,
    pub nt_iterations: usize,
    pub run_config_id: Option<String>,
    pub nt_version: String,
    pub created_at: String,
}

impl PublishedCatalogProof {
    fn validate_against(
        &self,
        metadata: &ConversionCatalogMetadata,
        contract: &BacktestResultContract,
        spec: &RunSpec,
    ) -> Result<()> {
        ensure!(
            self.proof_version == PUBLISHED_CATALOG_PROOF_VERSION,
            "unexpected published-catalog proof version"
        );
        let receipt = metadata
            .hydrated_publication_receipt()
            .context("published-catalog proof requires hydrated publication metadata")?;
        let (catalog_scheme, _) = self
            .catalog_uri
            .split_once("://")
            .context("published-catalog proof catalog URI is missing its scheme")?;
        ensure!(
            catalog_scheme == "s3" && self.catalog_fs_protocol == catalog_scheme,
            "published-catalog proof must bind the exact S3 catalog scheme"
        );
        ensure!(
            self.catalog_uri == receipt.catalog_root_uri
                && self.catalog_uri == contract.artifact_uris.nt_catalog_uri
                && self.publication_receipt_uri == receipt.receipt_uri
                && contract.artifact_uris.nt_catalog_manifest_uri.as_deref()
                    == Some(receipt.receipt_uri.as_str())
                && self.publication_receipt_sha256 == receipt.receipt_sha256
                && self.publication_receipt_version_id == receipt.receipt_version_id
                && self.publication_physical_manifest_sha256 == receipt.physical_manifest_sha256,
            "published-catalog proof receipt identity does not match metadata and result contract"
        );
        let expected_iterations = u64::try_from(self.expected_iterations)
            .context("published-catalog expected iterations do not fit u64")?;
        let nt_iterations = u64::try_from(self.nt_iterations)
            .context("published-catalog NT iterations do not fit u64")?;
        ensure!(
            expected_iterations == contract.nt_result.iterations
                && nt_iterations == contract.nt_result.iterations
                && self.run_config_id == contract.nt_result.run_config_id,
            "published-catalog proof result identity does not match the result contract"
        );
        ensure!(
            self.nt_version == contract.nt_version
                && self.nt_version == spec.manifest.resolved_nt_version
                && self.created_at == contract.created_at
                && self.created_at == spec.created_at_utc,
            "published-catalog proof version or creation identity does not match the result contract and RunSpec"
        );
        Ok(())
    }
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
    artifact_root: &ResolvedArtifactRoot,
    local_path: &Path,
    uri: &str,
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<DurableObjectVersionIdentity> {
    let path = guarded_operation_outcome(work_budget, OperatorWorkBudgetStage::Publish, || {
        artifact_root.object_path_for_uri(uri)
    })??;
    let payload = read_file_with_budget(local_path, work_budget, OperatorWorkBudgetStage::Publish)
        .with_context(|| {
            format!(
                "read durable contract artifact {} for {}",
                local_path.display(),
                uri
            )
        })?;
    let byte_len = u64::try_from(payload.len())
        .context("durable contract artifact length does not fit u64")?;
    let sha256 = sha256_hex_with_budget(&payload, work_budget, OperatorWorkBudgetStage::Publish)?;
    let version = writer
        .put_create_idempotent_guarded(&path, payload, work_budget)
        .await
        .with_context(|| format!("persist durable contract artifact {uri}"))?;
    let (version_id, e_tag) =
        required_versioned_create_result(version, &format!("durable artifact {uri}"))?;
    work_budget.check_deadline(OperatorWorkBudgetStage::Publish)?;
    Ok(DurableObjectVersionIdentity {
        uri: uri.to_string(),
        sha256,
        byte_len,
        version_id,
        e_tag,
    })
}

async fn persist_durable_contract_artifacts(
    writer: &CreateOnlyArtifactWriter<'_>,
    artifact_root: &ResolvedArtifactRoot,
    artifacts: &RunArtifacts,
    output_prefix: &str,
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<DurableCompletionArtifacts> {
    let uris = &artifacts.output.contract.artifact_uris;
    persist_durable_contract_artifact(
        writer,
        artifact_root,
        &artifacts.proof_path,
        &uris.source_proof_uri,
        work_budget,
    )
    .await?;
    ensure!(
        artifacts
            .output
            .conversion_catalog_metadata
            .catalog_consumption_proven(),
        "durable terminal manifest requires published catalog consumption proof"
    );
    let published_catalog_proof = persist_durable_contract_artifact(
        writer,
        artifact_root,
        &artifacts
            .catalog_metadata_path
            .with_file_name(PUBLISHED_CATALOG_PROOF_FILE),
        &portable_artifact_uri(output_prefix, PUBLISHED_CATALOG_PROOF_FILE),
        work_budget,
    )
    .await?;
    persist_durable_contract_artifact(
        writer,
        artifact_root,
        &artifacts.canonical_artifact_path,
        &uris.canonical_table_uri,
        work_budget,
    )
    .await?;
    let catalog_metadata = persist_durable_contract_artifact(
        writer,
        artifact_root,
        &artifacts.catalog_metadata_path,
        &uris.catalog_metadata_uri,
        work_budget,
    )
    .await?;
    let result_contract = persist_durable_contract_artifact(
        writer,
        artifact_root,
        &artifacts.contract_path,
        &uris.result_contract_uri,
        work_budget,
    )
    .await?;
    persist_durable_contract_artifact(
        writer,
        artifact_root,
        &artifacts.run_manifest_path,
        &portable_artifact_uri(output_prefix, BACKTEST_RUN_MANIFEST_FILE),
        work_budget,
    )
    .await?;
    let catalog_run_view_authority = persist_durable_contract_artifact(
        writer,
        artifact_root,
        &artifacts.catalog_run_view_authority_path,
        &portable_artifact_uri(output_prefix, CATALOG_RUN_VIEW_AUTHORITY_FILE),
        work_budget,
    )
    .await?;
    persist_durable_contract_artifact(
        writer,
        artifact_root,
        &artifacts.conversion_manifest_path,
        &portable_artifact_uri(
            output_prefix,
            crate::conversion_boundary::CONVERSION_MANIFEST_FILE,
        ),
        work_budget,
    )
    .await?;
    Ok(DurableCompletionArtifacts {
        result_contract,
        catalog_metadata,
        published_catalog_proof,
        catalog_run_view_authority,
    })
}

async fn read_pinned_durable_object_guarded(
    store: &dyn ObjectStore,
    path: &object_store::path::Path,
    byte_len: u64,
    version_id: &str,
    e_tag: Option<&str>,
    label: &str,
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<Vec<u8>> {
    let options = object_store::GetOptions {
        version: Some(version_id.to_string()),
        if_match: e_tag.map(str::to_string),
        ..object_store::GetOptions::default()
    };
    let result = guarded_async_operation_outcome(
        work_budget,
        OperatorWorkBudgetStage::ObjectVerification,
        store.get_opts(path, options),
    )
    .await?
    .with_context(|| format!("get exact version {} of {label} at {}", version_id, path))?;
    ensure!(
        result.meta.location == *path
            && result.meta.size == byte_len
            && result.range.start == 0
            && result.range.end == byte_len
            && result.meta.version.as_deref() == Some(version_id),
        "{label} exact-version response metadata mismatch"
    );
    if let Some(e_tag) = e_tag {
        ensure!(
            result.meta.e_tag.as_deref() == Some(e_tag),
            "{label} exact-version ETag mismatch"
        );
    }
    let mut output = ExactSizedObjectBuffer::new(byte_len)?;
    let mut stream = result.into_stream();
    loop {
        let chunk = guarded_async_operation_outcome(
            work_budget,
            OperatorWorkBudgetStage::ObjectVerification,
            async { stream.next().await.transpose() },
        )
        .await?
        .with_context(|| format!("stream exact-version {label}"))?;
        let Some(chunk) = chunk else { break };
        output.push(
            &chunk,
            work_budget,
            OperatorWorkBudgetStage::ObjectVerification,
        )?;
    }
    output.finish(work_budget, OperatorWorkBudgetStage::ObjectVerification)
}

async fn read_exact_durable_object_guarded(
    store: &dyn ObjectStore,
    artifact_root: &ResolvedArtifactRoot,
    identity: &DurableObjectVersionIdentity,
    label: &str,
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<Vec<u8>> {
    identity.validate(label)?;
    ensure!(
        identity.byte_len <= artifact_root.max_final_object_bytes(),
        "{label} exceeds artifact_store.max_final_object_bytes"
    );
    let path = artifact_root.object_path_for_uri(&identity.uri)?;
    let bytes = read_pinned_durable_object_guarded(
        store,
        &path,
        identity.byte_len,
        &identity.version_id,
        identity.e_tag.as_deref(),
        label,
        work_budget,
    )
    .await?;
    let actual_sha256 = sha256_hex_with_budget(
        &bytes,
        work_budget,
        OperatorWorkBudgetStage::ObjectVerification,
    )?;
    ensure!(
        actual_sha256 == identity.sha256,
        "{label} exact-version SHA-256 mismatch"
    );
    Ok(bytes)
}

/// Probe only the deterministic completion key. A genuine object-store
/// `NotFound` is the sole permission to execute source bytes. Any other HEAD
/// failure, missing/null version, or exact-version disagreement fails closed.
async fn discover_current_durable_completion_guarded(
    store: &dyn ObjectStore,
    artifact_root: &ResolvedArtifactRoot,
    spec: &RunSpec,
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<Option<(DurableCompletionLocator, Vec<u8>)>> {
    let uri = portable_artifact_uri(
        &spec.manifest.output_prefix,
        DURABLE_COMPLETION_MANIFEST_FILE,
    );
    let path = artifact_root.object_path_for_uri(&uri)?;
    let current = match guarded_async_operation_outcome(
        work_budget,
        OperatorWorkBudgetStage::ObjectVerification,
        store.head(&path),
    )
    .await?
    {
        Ok(current) => current,
        Err(object_store::Error::NotFound { .. }) => return Ok(None),
        Err(error) => {
            return Err(anyhow::Error::new(error).context(format!(
                "discover deterministic durable completion key {uri}"
            )));
        }
    };
    ensure!(
        current.location == path,
        "current durable completion location mismatch"
    );
    ensure!(
        current.size > 0 && current.size <= artifact_root.max_final_object_bytes(),
        "current durable completion byte length is invalid"
    );
    let version_id = current
        .version
        .as_deref()
        .context("current durable completion has no S3 version ID")?;
    ensure_immutable_s3_version_id("current durable completion S3 version ID", version_id)?;
    if let Some(e_tag) = current.e_tag.as_deref() {
        ensure!(
            !e_tag.is_empty(),
            "current durable completion ETag is empty"
        );
    }
    let bytes = read_pinned_durable_object_guarded(
        store,
        &path,
        current.size,
        version_id,
        current.e_tag.as_deref(),
        "current durable completion manifest",
        work_budget,
    )
    .await?;
    let sha256 = sha256_hex_with_budget(
        &bytes,
        work_budget,
        OperatorWorkBudgetStage::ObjectVerification,
    )?;
    let locator = DurableCompletionLocator {
        object: DurableObjectVersionIdentity {
            uri,
            sha256,
            byte_len: current.size,
            version_id: version_id.to_string(),
            e_tag: current.e_tag,
        },
    };
    locator.validate()?;
    Ok(Some((locator, bytes)))
}

struct DurableCompletionManifestValidation<'a> {
    spec: &'a RunSpec,
    fingerprint: &'a ConversionFingerprint,
    artifact_root: &'a ResolvedArtifactRoot,
    catalog_dispatch: &'a CatalogDispatchConfig,
    store: &'a dyn ObjectStore,
    work_budget: &'a OperatorWorkBudgetGuard,
}

async fn validate_durable_completion_manifest_bytes_guarded(
    validation: DurableCompletionManifestValidation<'_>,
    locator: &DurableCompletionLocator,
    manifest_bytes: Vec<u8>,
) -> Result<DurableRunReceipt> {
    let DurableCompletionManifestValidation {
        spec,
        fingerprint,
        artifact_root,
        catalog_dispatch,
        store,
        work_budget,
    } = validation;
    locator.object.validate("durable completion manifest")?;
    ensure!(
        locator.object.uri
            == portable_artifact_uri(
                &spec.manifest.output_prefix,
                DURABLE_COMPLETION_MANIFEST_FILE,
            ),
        "durable completion locator URI does not match the submitted run"
    );
    let manifest: DurableCompletionManifest = {
        let manifest: DurableCompletionManifest = deserialize_json_with_budget(
            &manifest_bytes,
            work_budget,
            OperatorWorkBudgetStage::ObjectVerification,
        )
        .context("parse durable completion manifest")?;
        let canonical_manifest_bytes =
            crate::reference_artifact::canonical_json_bytes(&manifest)
                .context("serialize canonical durable completion manifest")?;
        ensure!(
            canonical_manifest_bytes == manifest_bytes,
            "durable completion manifest bytes are not canonical"
        );
        manifest
    };

    // Fetch sequentially so the retry path retains at most one exact small
    // artifact payload at a time before parsing it into its typed form.
    let receipt = {
        let bytes = read_exact_durable_object_guarded(
            store,
            artifact_root,
            &manifest.publication_receipt,
            "catalog publication receipt",
            work_budget,
        )
        .await?;
        CatalogProjectionPublicationReceipt::parse_and_validate_guarded(
            &bytes,
            &manifest.publication_receipt.sha256,
            work_budget,
            OperatorWorkBudgetStage::ObjectVerification,
        )?
    };
    let contract: BacktestResultContract = {
        let bytes = read_exact_durable_object_guarded(
            store,
            artifact_root,
            &manifest.result_contract,
            "result contract",
            work_budget,
        )
        .await?;
        deserialize_json_with_budget(
            &bytes,
            work_budget,
            OperatorWorkBudgetStage::ObjectVerification,
        )
        .context("parse exact-version result contract")?
    };
    validate_durable_result_contract_cross_claims(&contract, spec, fingerprint, &manifest)
        .context("cross-validate exact-version result contract")?;
    let metadata: ConversionCatalogMetadata = {
        let bytes = read_exact_durable_object_guarded(
            store,
            artifact_root,
            &manifest.catalog_metadata,
            "catalog metadata",
            work_budget,
        )
        .await?;
        deserialize_json_with_budget(
            &bytes,
            work_budget,
            OperatorWorkBudgetStage::ObjectVerification,
        )
        .context("parse exact-version catalog metadata")?
    };
    let proof: PublishedCatalogProof = {
        let bytes = read_exact_durable_object_guarded(
            store,
            artifact_root,
            &manifest.published_catalog_proof,
            "published catalog proof",
            work_budget,
        )
        .await?;
        deserialize_json_with_budget(
            &bytes,
            work_budget,
            OperatorWorkBudgetStage::ObjectVerification,
        )
        .context("parse exact-version published catalog proof")?
    };
    proof.validate_against(&metadata, &contract, spec)?;
    let authority: CatalogRunViewAuthority = {
        let bytes = read_exact_durable_object_guarded(
            store,
            artifact_root,
            &manifest.catalog_run_view_authority,
            "catalog run-view authority",
            work_budget,
        )
        .await?;
        let authority: CatalogRunViewAuthority = deserialize_json_with_budget(
            &bytes,
            work_budget,
            OperatorWorkBudgetStage::ObjectVerification,
        )
        .context("parse exact-version catalog run-view authority")?;
        let submitted_identity = submitted_run_identity_for_spec(spec)?;
        let canonical_authority = authority.canonical_bytes_guarded(
            &spec.manifest,
            &submitted_identity,
            work_budget,
            OperatorWorkBudgetStage::ObjectVerification,
        )?;
        ensure!(
            canonical_authority == bytes,
            "catalog run-view authority bytes are not canonical"
        );
        authority
    };
    let [authority_root] = authority.roots.as_slice() else {
        bail!("durable completion requires exactly one catalog run-view root")
    };
    ensure!(
        authority_root.physical_manifest == receipt.physical_manifest
            && authority_root.physical_manifest_sha256 == receipt.physical_manifest_sha256
            && authority_root.logical_catalog_hash == manifest.catalog_hash,
        "durable completion receipt and run-view authority disagree"
    );
    let expected_catalog_root = catalog_dispatch.catalog_root_for(
        &spec.source_proof.source_binding,
        spec.manifest.market_structure_fixture,
        artifact_root,
    )?;
    ensure!(
        receipt.catalog_root_uri == expected_catalog_root
            && receipt.binding.source_binding == spec.source_proof.source_binding,
        "durable completion receipt does not match source-binding dispatch"
    );
    let receipt_identity = metadata
        .hydrated_publication_receipt()
        .context("durable completion metadata lacks hydrated receipt identity")?;
    ensure!(
        receipt_identity.receipt_uri == manifest.publication_receipt.uri
            && receipt_identity.receipt_sha256 == manifest.publication_receipt.sha256
            && receipt_identity.receipt_version_id == manifest.publication_receipt.version_id
            && receipt_identity.physical_manifest_sha256 == receipt.physical_manifest_sha256
            && contract.catalog_hash == manifest.catalog_hash
            && metadata.catalog_hash == manifest.catalog_hash
            && contract.catalog_metadata_hash == metadata.content_hash()?
            && u64::try_from(metadata.canonical_rows)
                .context("catalog metadata canonical rows do not fit u64")?
                == manifest.canonical_rows
            && contract.nt_result.iterations == manifest.nt_catalog_rows,
        "durable completion artifact claims do not match terminal summary"
    );
    Ok(DurableRunReceipt {
        completion: locator.clone(),
        run_id: manifest.run_id,
        submitted_manifest_hash: manifest.submitted_manifest_hash,
        canonical_rows: manifest.canonical_rows,
        nt_catalog_rows: manifest.nt_catalog_rows,
        catalog_hash: manifest.catalog_hash,
    })
}

fn verify_completed_result_contract(
    path: &Path,
    contract: &BacktestResultContract,
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<BacktestResultContract> {
    let existing = read_json_artifact_guarded::<BacktestResultContract>(
        path,
        work_budget,
        OperatorWorkBudgetStage::Finalize,
    )?;
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

/// Opaque, hash-before-parse source-control snapshot used by every executable
/// operator entry. Its fields are private so callers cannot pair arbitrary
/// parsed registry values with a RunSpec identity.
#[derive(Debug, Clone)]
pub struct VerifiedSourceBindingRegistry {
    declared_path: PathBuf,
    resolved_path: PathBuf,
    sha256: String,
    _bytes: Arc<[u8]>,
    registry: Arc<SourceBindingRegistry>,
}

impl VerifiedSourceBindingRegistry {
    /// Read, hash, and only then parse the registry named by `spec`.
    ///
    /// # Errors
    ///
    /// Returns an error when the configured path cannot be resolved/read or the
    /// snapshotted bytes are not a valid source-binding registry.
    pub fn from_run_spec(spec: &RunSpec) -> Result<Self> {
        Self::from_run_spec_guarded(spec, &OperatorWorkBudgetGuard::unbounded())
    }

    /// Snapshot the registry through the caller's execution-plan byte and wall
    /// budget. Direct executable entrypoints use this before parsing registry
    /// TOML, so a large or stalled control file cannot bypass the run budget.
    pub fn from_run_spec_guarded(
        spec: &RunSpec,
        work_budget: &OperatorWorkBudgetGuard,
    ) -> Result<Self> {
        // Direct invocation trusts the exact bytes visible at this boundary;
        // immutable run evidence binds the digest computed from this snapshot.
        let resolved = resolve_active_backfill_runtime_input(None, &spec.source_bindings_path)
            .with_context(|| {
                format!(
                    "guard source-bindings registry {}",
                    spec.source_bindings_path.display()
                )
            })?
            .canonicalize()
            .with_context(|| {
                format!(
                    "resolve source-bindings registry {}",
                    spec.source_bindings_path.display()
                )
            })?;
        let bytes = read_file_with_budget(
            &resolved,
            work_budget,
            OperatorWorkBudgetStage::ObjectVerification,
        )
        .with_context(|| format!("read source-bindings registry {}", resolved.display()))?;
        Self::from_exact_bytes(spec, resolved, Arc::from(bytes))
    }

    /// Construct the same opaque snapshot from bytes already frozen and
    /// hash-checked at the execution-pack boundary. This remains crate-private:
    /// pack resolution is the sole owner of portable-path containment.
    pub(crate) fn from_frozen_pack_bytes(
        spec: &RunSpec,
        resolved_path: PathBuf,
        bytes: Arc<[u8]>,
        expected_sha256: &str,
    ) -> Result<Self> {
        // Production pack execution has an independent caller pin: the frozen
        // bytes must also match the v4 execution-record digest before parsing.
        ensure!(
            resolved_path.is_absolute(),
            "resolved source-bindings pack path must be absolute: {}",
            resolved_path.display()
        );
        ensure!(
            is_lowercase_sha256_hex(expected_sha256),
            "execution-pack source_bindings_sha256 must be 64 lowercase-hex characters"
        );
        let verified = Self::from_exact_bytes(spec, resolved_path.clone(), bytes)?;
        ensure!(
            verified.sha256 == expected_sha256,
            "source-bindings registry {} SHA-256 mismatch: expected {}, got {}",
            resolved_path.display(),
            expected_sha256,
            verified.sha256
        );
        Ok(verified)
    }

    fn from_exact_bytes(spec: &RunSpec, resolved_path: PathBuf, bytes: Arc<[u8]>) -> Result<Self> {
        let actual_sha256 = hex::encode(Sha256::digest(bytes.as_ref()));
        let text = std::str::from_utf8(bytes.as_ref()).with_context(|| {
            format!(
                "decode source-bindings registry {} as UTF-8",
                resolved_path.display()
            )
        })?;
        let registry = SourceBindingRegistry::from_toml_str(text).with_context(|| {
            format!("parse source-bindings registry {}", resolved_path.display())
        })?;
        Ok(Self {
            declared_path: spec.source_bindings_path.clone(),
            resolved_path,
            sha256: actual_sha256,
            _bytes: bytes,
            registry: Arc::new(registry),
        })
    }

    pub(crate) fn reassert_for(&self, spec: &RunSpec) -> Result<()> {
        ensure!(
            self.declared_path == spec.source_bindings_path,
            "verified source-bindings path mismatch: handle {:?}, run spec {:?}",
            self.declared_path,
            spec.source_bindings_path
        );
        Ok(())
    }

    fn registry(&self) -> &SourceBindingRegistry {
        self.registry.as_ref()
    }

    pub(crate) fn resolved_path(&self) -> &Path {
        &self.resolved_path
    }

    pub(crate) fn sha256(&self) -> &str {
        &self.sha256
    }
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
    let mut manifest = spec.manifest.clone();
    bind_runtime_manifest_to_local_catalog_root(&mut manifest, &output_dir.join(CATALOG_DIR))?;
    Ok(manifest)
}

fn bind_runtime_manifest_to_local_catalog_root(
    manifest: &mut BacktestingRunManifest,
    catalog_root: &Path,
) -> Result<()> {
    let catalog_path = catalog_root
        .to_str()
        .context("catalog path is not valid UTF-8")?
        .to_string();
    {
        let catalog_input = manifest.single_catalog_input_mut().map_err(|error| {
            anyhow::anyhow!("local catalog manifest requires one catalog input: {error}")
        })?;
        catalog_input.catalog_path = catalog_path;
        catalog_input.catalog_fs_protocol = CATALOG_FS_PROTOCOL_NONE.to_string();
        catalog_input.catalog_fs_storage_options.clear();
        catalog_input.catalog_fs_rust_storage_options.clear();
    }
    Ok(())
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
    let registry = VerifiedSourceBindingRegistry::from_run_spec(spec)?;
    validate_run_spec_manifest_for_object_hash_with_verified_registry(
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
pub fn validate_run_spec_manifest_for_object_hash_with_verified_registry(
    spec: &RunSpec,
    output_dir: &Path,
    object_sha256: &str,
    registry: &VerifiedSourceBindingRegistry,
) -> Result<()> {
    registry.reassert_for(spec)?;
    validate_converter_config(&spec.converter)?;
    let (_, accepted) =
        accepted_dataset_for_run_spec_hash_with_registry(spec, object_sha256, registry.registry())?;
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

fn read_limited_bytes<R: Read>(
    reader: R,
    max_decoded_bytes: u64,
    context_label: &str,
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<Vec<u8>> {
    let read_limit = max_decoded_bytes
        .checked_add(1)
        .context("converter.raw_payload.max_decoded_bytes is too large")?;
    let mut limited =
        CooperativeDeadlineReader::new(reader, work_budget, OperatorWorkBudgetStage::Decode)
            .take(read_limit);
    // Grow from bytes actually observed. The declared ceiling may be several
    // GiB and is a rejection bound, never an allocation request.
    let mut bytes = Vec::new();
    limited
        .read_to_end(&mut bytes)
        .with_context(|| format!("decode {context_label}"))?;
    let decoded_len = u64::try_from(bytes.len()).context("decoded byte length does not fit u64")?;
    ensure!(
        decoded_len <= max_decoded_bytes,
        "decoded text byte length {} exceeds converter.raw_payload.max_decoded_bytes {max_decoded_bytes}",
        bytes.len()
    );
    Ok(bytes)
}

fn read_limited_csv_text<R: Read>(
    reader: R,
    max_decoded_bytes: u64,
    context_label: &str,
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<String> {
    let bytes = read_limited_bytes(reader, max_decoded_bytes, context_label, work_budget)?;
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
    ParquetBytes(Bytes),
}

fn decode_object_payload(
    config: &RawPayloadConfig,
    object_bytes: &[u8],
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<DecodedPayload> {
    guarded_operation_outcome(
        work_budget,
        OperatorWorkBudgetStage::Decode,
        || -> Result<_> {
            validate_raw_payload_config(config)?;
            match config.container {
                RawPayloadContainer::CsvGzip => Ok(DecodedPayload::Text(read_limited_csv_text(
                    flate2::read::GzDecoder::new(object_bytes),
                    config.max_decoded_bytes,
                    "gzip csv object",
                    work_budget,
                )?)),
                RawPayloadContainer::CsvText => Ok(DecodedPayload::Text(read_limited_csv_text(
                    Cursor::new(object_bytes),
                    config.max_decoded_bytes,
                    "plain csv object",
                    work_budget,
                )?)),
                RawPayloadContainer::JsonlText => Ok(DecodedPayload::Text(read_limited_csv_text(
                    Cursor::new(object_bytes),
                    config.max_decoded_bytes,
                    "plain jsonl object",
                    work_budget,
                )?)),
                RawPayloadContainer::JsonlGzip => Ok(DecodedPayload::Text(read_limited_csv_text(
                    flate2::read::GzDecoder::new(object_bytes),
                    config.max_decoded_bytes,
                    "gzip jsonl object",
                    work_budget,
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
                        work_budget,
                    )?;
                    work_budget.check_deadline(OperatorWorkBudgetStage::Decode)?;
                    member.verify().context("verify jsonl zip member")?;
                    Ok(DecodedPayload::Text(text))
                }
                RawPayloadContainer::SingleCsvZip => {
                    let member_name = config.zip_member.as_deref().context(
                        "converter.raw_payload.zip_member is required for single_csv_zip",
                    )?;
                    let cursor = Cursor::new(object_bytes);
                    let mut archive =
                        zip::ZipArchive::new(cursor).context("open zip csv object")?;
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
                        work_budget,
                    )?))
                }
                RawPayloadContainer::TarGzipJsonl => {
                    let member_suffix = config.member_suffix.as_deref().context(
                        "converter.raw_payload.member_suffix is required for tar_gzip_jsonl",
                    )?;
                    let max_member_bytes = config.max_member_bytes.context(
                        "converter.raw_payload.max_member_bytes is required for tar_gzip_jsonl",
                    )?;
                    let mut members = Vec::new();
                    let mut retained_member_bytes = 0_u64;
                    let decoder = flate2::read::MultiGzDecoder::new(Cursor::new(object_bytes));
                    let reader = CooperativeDeadlineReader::new(
                        decoder,
                        work_budget,
                        OperatorWorkBudgetStage::Decode,
                    );
                    for member in
                        crate::tar_reader::tar_members(reader, member_suffix, max_member_bytes)
                    {
                        work_budget.check_deadline(OperatorWorkBudgetStage::Decode)?;
                        let member = member.context("stream gzip tar jsonl member")?;
                        let member_bytes = u64::try_from(member.text.len())
                            .context("tar JSONL member length does not fit u64")?;
                        retained_member_bytes = retained_member_bytes
                            .checked_add(member_bytes)
                            .context("cumulative tar JSONL member length overflow")?;
                        ensure!(
                            retained_member_bytes <= config.max_decoded_bytes,
                            "cumulative tar JSONL member bytes {retained_member_bytes} exceed converter.raw_payload.max_decoded_bytes {}",
                            config.max_decoded_bytes
                        );
                        work_budget.verify_decoded_bytes(
                            retained_member_bytes,
                            OperatorWorkBudgetStage::Decode,
                        )?;
                        members.try_reserve(1).map_err(|error| {
                            anyhow::anyhow!("reserve tar JSONL member: {error}")
                        })?;
                        members.push(member);
                        work_budget.check_deadline(OperatorWorkBudgetStage::Decode)?;
                    }
                    Ok(DecodedPayload::TarMembers(members))
                }
                RawPayloadContainer::ParquetFile => Ok(DecodedPayload::ParquetBytes(Bytes::from(
                    read_limited_bytes(
                        Cursor::new(object_bytes),
                        config.max_object_bytes,
                        "parquet object",
                        work_budget,
                    )?,
                ))),
            }
        },
    )?
}

struct CompletedOutputInputs<'a> {
    spec: &'a RunSpec,
    verified_sha256: String,
    accepted_source_proof: SourceProofReport,
    accepted: &'a AcceptedDataset,
    conversion_fingerprint: &'a ConversionFingerprint,
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
    work_budget: &'a OperatorWorkBudgetGuard,
}

fn run_from_completed_output(inputs: CompletedOutputInputs<'_>) -> Result<RunArtifacts> {
    let seal: OperatorTerminalSeal = read_json_artifact_guarded(
        &inputs
            .catalog_root
            .parent()
            .context("completed trade catalog root has no output parent")?
            .join(OPERATOR_TERMINAL_SEAL_FILE),
        inputs.work_budget,
        OperatorWorkBudgetStage::Finalize,
    )?;
    let output_dir = inputs
        .catalog_root
        .parent()
        .context("completed trade catalog root has no output parent")?;
    let current_files = collect_operator_terminal_seal_files(output_dir, inputs.work_budget)?;
    let sealed_summary =
        verify_completed_operator_output_against_seal(CompletedOperatorOutputVerification {
            spec: inputs.spec,
            expected_source_proof: &inputs.accepted_source_proof,
            accepted: inputs.accepted,
            fingerprint: inputs.conversion_fingerprint,
            output_dir,
            seal: &seal,
            current_files: &current_files,
            verify_physical_catalog_view: false,
            work_budget: inputs.work_budget,
        })?;
    inputs
        .work_budget
        .check_deadline(OperatorWorkBudgetStage::CatalogProjection)?;
    let conversion_checkpoint: ConversionCheckpoint = read_json_artifact_guarded(
        &inputs.conversion_checkpoint_path,
        inputs.work_budget,
        OperatorWorkBudgetStage::CatalogProjection,
    )?;
    ensure!(
        conversion_checkpoint.content_hash()? == inputs.conversion_checkpoint_hash,
        "completed conversion checkpoint hash changed after inspection"
    );
    inputs
        .work_budget
        .check_deadline(OperatorWorkBudgetStage::CatalogProjection)?;
    let conversion_manifest: ConversionManifest = read_json_artifact_guarded(
        &inputs.conversion_manifest_path,
        inputs.work_budget,
        OperatorWorkBudgetStage::CatalogProjection,
    )?;
    ensure!(
        conversion_manifest.content_hash()? == inputs.conversion_manifest_hash,
        "completed conversion manifest hash changed after inspection"
    );
    ensure!(
        conversion_manifest.output_catalog_uri == inputs.artifact_uris.nt_catalog_uri,
        "completed conversion output_catalog_uri does not match current run manifest"
    );
    inputs
        .work_budget
        .check_deadline(OperatorWorkBudgetStage::CatalogProjection)?;
    let conversion_catalog_metadata: ConversionCatalogMetadata = read_json_artifact_guarded(
        &inputs.catalog_metadata_path,
        inputs.work_budget,
        OperatorWorkBudgetStage::CatalogProjection,
    )?;
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
    inputs
        .work_budget
        .check_deadline(OperatorWorkBudgetStage::CatalogProjection)?;

    let actual_catalog_hash =
        logical_catalog_hash_guarded(&inputs.catalog_root, inputs.work_budget)
            .with_context(|| format!("verify catalog hash {}", inputs.catalog_root.display()))?;
    ensure!(
        actual_catalog_hash == inputs.expected_catalog_hash,
        "completed NT catalog hash mismatch: expected {:?}, got {:?}",
        inputs.expected_catalog_hash,
        actual_catalog_hash
    );
    inputs
        .work_budget
        .check_deadline(OperatorWorkBudgetStage::CatalogProjection)?;
    let canonical_table = CanonicalTradesTable::read_parquet_guarded(
        &inputs.canonical_artifact_path,
        inputs.accepted,
        inputs.work_budget,
    )?;
    let actual_metadata =
        actual_nt_market_data_metadata_guarded(&inputs.catalog_root, inputs.work_budget)?;
    inputs
        .work_budget
        .check_deadline(OperatorWorkBudgetStage::CatalogProjection)?;
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
    assert_time_window_overlaps_data_guarded(
        &inputs.manifest,
        &canonical_table,
        inputs.work_budget,
    )?;

    let read_back = read_back_trade_ticks_guarded(
        &inputs.catalog_root,
        &conversion_manifest.nt_instrument_id,
        inputs.work_budget,
    )
    .context("read back completed NT catalog")?;
    inputs
        .work_budget
        .check_deadline(OperatorWorkBudgetStage::CatalogProjection)?;
    ensure!(
        read_back.len() == canonical_table.rows.len(),
        "completed NT catalog read-back count {} does not match canonical rows {}",
        read_back.len(),
        canonical_table.rows.len()
    );
    assert_read_back_matches_guarded(
        &read_back,
        &canonical_table.rows,
        &conversion_manifest.nt_instrument_id,
        inputs.work_budget,
    )?;

    let submitted_identity = submitted_run_identity_for_spec(inputs.spec)?;
    let catalog_run_view_authority = load_catalog_run_view_authority_guarded(
        inputs.spec,
        &inputs.manifest,
        output_dir,
        inputs.work_budget,
    )?;

    let crate::runner::NtBacktestNodeRun {
        result: nt_result,
        order_terminals,
        config_override_report,
        run_guard_report,
        ..
    } = run_nt_backtest_node_guarded(
        &inputs.manifest,
        &submitted_identity,
        &catalog_run_view_authority,
        inputs.work_budget,
    )?;
    inputs
        .work_budget
        .check_deadline(OperatorWorkBudgetStage::Backtest)?;
    let expected = expected_iterations_guarded(
        &canonical_table.rows,
        inputs.manifest.start_time,
        inputs.manifest.end_time,
        inputs.work_budget,
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
        catalog_run_view_authority,
        conversion_checkpoint,
        conversion_manifest,
        conversion_catalog_metadata,
        conversion_checkpoint_hash: inputs.conversion_checkpoint_hash,
        conversion_manifest_hash: inputs.conversion_manifest_hash,
        read_back_count,
        expected_iterations: expected,
        nt_result,
        order_terminals,
        contract,
    };
    redact_operator_contract(&mut output, &inputs.catalog_root);
    output.contract = verify_completed_result_contract(
        &inputs.contract_path,
        &output.contract,
        inputs.work_budget,
    )?;
    inputs
        .work_budget
        .check_deadline(OperatorWorkBudgetStage::Finalize)?;

    let batch_summary = OperatorRunSummary::trade(&output)?;
    ensure!(
        batch_summary == sealed_summary,
        "completed trade summary changed after sealed verification"
    );
    let catalog_run_view_authority_path = output_dir.join(CATALOG_RUN_VIEW_AUTHORITY_FILE);
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
        catalog_run_view_authority_path,
        canonical_catalog_uri: None,
        persisted_catalog_projection: None,
        persisted_catalog_objects: Vec::new(),
        output,
        batch_summary,
        transient_catalog_root_lease: None,
    })
}

fn read_json_artifact_guarded<T: DeserializeOwned>(
    path: &Path,
    work_budget: &OperatorWorkBudgetGuard,
    stage: OperatorWorkBudgetStage,
) -> Result<T> {
    let bytes = read_file_with_budget(path, work_budget, stage)?;
    deserialize_json_with_budget(&bytes, work_budget, stage)
        .with_context(|| format!("parse {}", path.display()))
}

fn run_budgeted_stage<T>(
    work_budget: &OperatorWorkBudgetGuard,
    stage: OperatorWorkBudgetStage,
    operation: impl FnOnce() -> Result<T>,
) -> Result<T> {
    crate::operator_work_budget::guarded_operation_outcome(work_budget, stage, operation)?
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
    let local_spec = LocalRunSpec::new(spec)?;
    let registry = VerifiedSourceBindingRegistry::from_run_spec_guarded(spec, work_budget)?;
    run_from_local_run_spec_with_verified_registry(
        local_spec,
        object_bytes,
        output_dir,
        &registry,
        work_budget,
    )
}

fn run_from_local_run_spec_with_verified_registry(
    local_spec: LocalRunSpec<'_>,
    object_bytes: &[u8],
    output_dir: &Path,
    registry: &VerifiedSourceBindingRegistry,
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<RunArtifacts> {
    let spec = local_spec.get();
    registry.reassert_for(spec)?;
    run_from_run_spec_inner(local_spec, object_bytes, output_dir, registry, work_budget)
}

/// Unit-test adapter for registry-identity tests. Production callers cannot
/// bypass [`LocalRunSpec::new`].
#[cfg(test)]
pub(crate) fn run_from_run_spec_with_verified_registry(
    spec: &RunSpec,
    object_bytes: &[u8],
    output_dir: &Path,
    registry: &VerifiedSourceBindingRegistry,
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<RunArtifacts> {
    run_from_local_run_spec_with_verified_registry(
        LocalRunSpec::new(spec)?,
        object_bytes,
        output_dir,
        registry,
        work_budget,
    )
}

enum TradeRunPreparation {
    Completed(Box<RunArtifacts>),
    Prepared(Box<PreparedTradeRunArtifacts>),
}

struct PreparedTradeRunArtifacts {
    verified_sha256: String,
    accepted_source_proof: SourceProofReport,
    accepted: AcceptedDataset,
    conversion_fingerprint: ConversionFingerprint,
    canonical_artifact_path: PathBuf,
    catalog_root: PathBuf,
    proof_path: PathBuf,
    contract_path: PathBuf,
    run_manifest_path: PathBuf,
    conversion_manifest_path: PathBuf,
    conversion_checkpoint_path: PathBuf,
    catalog_metadata_path: PathBuf,
    local_manifest: BacktestingRunManifest,
    contract_manifest_hash: String,
    artifact_uris: ResultArtifactUris,
    backtest: PreparedBacktestRun,
    transient_catalog_root_lease: Option<TransientCatalogRootLease>,
}

fn run_from_run_spec_inner(
    local_spec: LocalRunSpec<'_>,
    object_bytes: &[u8],
    output_dir: &Path,
    source_binding_registry: &VerifiedSourceBindingRegistry,
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<RunArtifacts> {
    let spec = local_spec.get();
    match prepare_local_run_from_run_spec_inner(
        local_spec,
        object_bytes,
        output_dir,
        source_binding_registry,
        work_budget,
    )? {
        TradeRunPreparation::Completed(artifacts) => Ok(*artifacts),
        TradeRunPreparation::Prepared(prepared) => {
            let runtime_manifest = prepared.local_manifest.clone();
            finalize_prepared_trade_run(
                spec,
                *prepared,
                runtime_manifest,
                output_dir,
                true,
                work_budget,
            )
        }
    }
}

fn prepare_local_run_from_run_spec_inner(
    local_spec: LocalRunSpec<'_>,
    object_bytes: &[u8],
    output_dir: &Path,
    source_binding_registry: &VerifiedSourceBindingRegistry,
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<TradeRunPreparation> {
    prepare_run_from_run_spec_inner(
        local_spec.get(),
        object_bytes,
        output_dir,
        true,
        source_binding_registry,
        work_budget,
    )
}

fn prepare_run_from_run_spec_inner(
    spec: &RunSpec,
    object_bytes: &[u8],
    output_dir: &Path,
    reuse_completed_output: bool,
    source_binding_registry: &VerifiedSourceBindingRegistry,
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<TradeRunPreparation> {
    source_binding_registry.reassert_for(spec)?;
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
    let verified_sha256 = sha256_hex_with_budget(
        object_bytes,
        work_budget,
        OperatorWorkBudgetStage::ObjectVerification,
    )?;
    ensure!(
        verified_sha256 == spec.accepted_object.sha256,
        "object SHA-256 {verified_sha256} does not match run-spec {}",
        spec.accepted_object.sha256
    );

    // Gate 1: accept the source proof and bind the object via the ledger.
    let (accepted_proof, accepted) = accepted_dataset_for_run_spec_hash_with_registry(
        spec,
        &verified_sha256,
        source_binding_registry.registry(),
    )?;
    validate_converter_table_family(&spec.converter, &accepted.table_family)?;

    let conversion_fingerprint =
        conversion_fingerprint_for(spec, &accepted, source_binding_registry)?;
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
    let mut manifest = local_run_manifest_for_output(spec, output_dir)?;
    validate_local_run_manifest(&manifest, &accepted)?;
    let artifact_uris = portable_artifact_uris(&manifest);

    work_budget.check_deadline(OperatorWorkBudgetStage::Decode)?;
    let DecodedPayload::Text(csv_text) =
        decode_object_payload(&spec.converter.raw_payload, object_bytes, work_budget)?
    else {
        anyhow::bail!(
            "single-table trade entry requires a text payload container, got {:?}",
            spec.converter.raw_payload.container
        );
    };
    work_budget.check_deadline(OperatorWorkBudgetStage::Decode)?;

    if reuse_completed_output {
        let sealed_completion = preflight_completed_output_before_inspection(
            spec,
            &accepted_proof,
            &accepted,
            output_dir,
            &conversion_fingerprint,
            work_budget,
        )?;
        match inspect_conversion_output(output_dir, &conversion_fingerprint)? {
            ConversionOutputState::Complete {
                manifest_hash,
                checkpoint_hash,
                catalog_hash,
            } if sealed_completion => {
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
                let completed_inputs = CompletedOutputInputs {
                    spec,
                    verified_sha256,
                    accepted_source_proof: accepted_proof,
                    accepted: &accepted,
                    conversion_fingerprint: &conversion_fingerprint,
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
                    work_budget,
                };
                return run_budgeted_stage(work_budget, OperatorWorkBudgetStage::Finalize, || {
                    run_from_completed_output(completed_inputs)
                        .map(Box::new)
                        .map(TradeRunPreparation::Completed)
                });
            }
            ConversionOutputState::Complete { .. }
            | ConversionOutputState::CleanNew
            | ConversionOutputState::ResumeFromCheckpoint { .. } => {}
        }
    }

    work_budget.check_deadline(OperatorWorkBudgetStage::CanonicalWrite)?;
    fs::create_dir_all(output_dir)
        .with_context(|| format!("create output dir {}", output_dir.display()))?;
    let (catalog_root, transient_catalog_root_lease) = if reuse_completed_output {
        (catalog_root, None)
    } else {
        let lease = TransientCatalogRootLease::acquire_stable(output_dir)?;
        let stable_catalog_root = lease.catalog_root.clone();
        (stable_catalog_root, Some(lease))
    };
    bind_runtime_manifest_to_local_catalog_root(&mut manifest, &catalog_root)?;
    validate_local_run_manifest(&manifest, &accepted)?;
    let conversion_control_artifact_path = spec
        .source_bindings_path
        .to_str()
        .context("source_bindings_path is not valid UTF-8")?;
    let submitted_identity = submitted_run_identity_for_spec(spec)?;
    let backtest_inputs = BacktestRunInputs {
        accepted: &accepted,
        identity,
        instrument_spec,
        csv_text: &csv_text,
        capture_time_nanos: rfc3339_to_nanos(&spec.capture_time_utc)?,
        manifest: &manifest,
        contract_manifest_hash: &contract_manifest_hash,
        converter: &spec.converter,
        conversion_control_artifact_path,
        conversion_control_artifact_sha256: source_binding_registry.sha256(),
        canonical_artifact_path: &canonical_path,
        catalog_root: &catalog_root,
        authoritative_output_root: output_dir,
        selector_provenance: None,
        created_at: &spec.created_at_utc,
        artifact_uris: artifact_uris.clone(),
        work_budget,
    };
    let backtest = prepare_backtest(&backtest_inputs, submitted_identity)?;
    Ok(TradeRunPreparation::Prepared(Box::new(
        PreparedTradeRunArtifacts {
            verified_sha256,
            accepted_source_proof: accepted_proof,
            accepted,
            conversion_fingerprint,
            canonical_artifact_path: canonical_path,
            catalog_root,
            proof_path,
            contract_path,
            run_manifest_path,
            conversion_manifest_path,
            conversion_checkpoint_path,
            catalog_metadata_path,
            local_manifest: manifest,
            contract_manifest_hash,
            artifact_uris,
            backtest,
            transient_catalog_root_lease,
        },
    )))
}

fn finalize_prepared_trade_run(
    spec: &RunSpec,
    prepared: PreparedTradeRunArtifacts,
    runtime_manifest: BacktestingRunManifest,
    output_dir: &Path,
    reuse_completed_output: bool,
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<RunArtifacts> {
    let runtime_catalog_root = PathBuf::from(
        runtime_manifest
            .single_catalog_input()
            .map_err(|error| anyhow::anyhow!(error))?
            .catalog_path
            .as_str(),
    );
    let mut output = execute_prepared_backtest(
        prepared.backtest,
        BacktestRunFinalizeInputs {
            accepted: &prepared.accepted,
            runtime_manifest: &runtime_manifest,
            contract_manifest_hash: &prepared.contract_manifest_hash,
            selector_provenance: None,
            created_at: &spec.created_at_utc,
            artifact_uris: prepared.artifact_uris,
            work_budget,
        },
    )?;
    persist_catalog_run_view_authority_guarded(
        spec,
        &runtime_manifest,
        &output.catalog_run_view_authority,
        output_dir,
        work_budget,
    )?;
    redact_operator_contract(&mut output, &runtime_catalog_root);

    let proof_bytes = serde_json::to_vec_pretty(&prepared.accepted_source_proof)
        .context("serialize accepted source proof")?;
    persist_immutable_local_bytes_guarded(
        &prepared.proof_path,
        &proof_bytes,
        "accepted source proof",
        work_budget,
        OperatorWorkBudgetStage::Finalize,
    )?;
    if reuse_completed_output {
        let contract_bytes =
            serde_json::to_vec_pretty(&output.contract).context("serialize result contract")?;
        persist_immutable_local_bytes_guarded(
            &prepared.contract_path,
            &contract_bytes,
            "result contract",
            work_budget,
            OperatorWorkBudgetStage::Finalize,
        )?;
    }
    let run_manifest_bytes = serde_json::to_vec_pretty(&spec.manifest.to_artifact_manifest()?)
        .context("serialize resolved run manifest")?;
    persist_immutable_local_bytes_guarded(
        &prepared.run_manifest_path,
        &run_manifest_bytes,
        "resolved run manifest",
        work_budget,
        OperatorWorkBudgetStage::Finalize,
    )?;
    let batch_summary = OperatorRunSummary::trade(&output)?;
    let artifacts = RunArtifacts {
        verified_sha256: prepared.verified_sha256,
        accepted_source_proof: prepared.accepted_source_proof,
        canonical_artifact_path: prepared.canonical_artifact_path,
        catalog_root: prepared.catalog_root,
        proof_path: prepared.proof_path,
        contract_path: prepared.contract_path,
        run_manifest_path: prepared.run_manifest_path,
        conversion_manifest_path: prepared.conversion_manifest_path,
        conversion_checkpoint_path: prepared.conversion_checkpoint_path,
        catalog_metadata_path: prepared.catalog_metadata_path,
        catalog_run_view_authority_path: output_dir.join(CATALOG_RUN_VIEW_AUTHORITY_FILE),
        canonical_catalog_uri: None,
        persisted_catalog_projection: None,
        persisted_catalog_objects: Vec::new(),
        output,
        batch_summary,
        transient_catalog_root_lease: prepared.transient_catalog_root_lease,
    };
    if reuse_completed_output {
        write_completed_conversion_artifacts_guarded(
            output_dir,
            &artifacts.output.conversion_manifest,
            &artifacts.output.conversion_checkpoint,
            &artifacts.output.conversion_catalog_metadata,
            work_budget,
        )?;
        commit_operator_terminal_seal(
            spec,
            &artifacts.accepted_source_proof,
            &prepared.accepted,
            &prepared.conversion_fingerprint,
            output_dir,
            &artifacts.batch_summary,
            work_budget,
        )?;
    }
    Ok(artifacts)
}

struct DurableRunSpecPreflight {
    artifact_root: ResolvedArtifactRoot,
    credential_parameters: NtCatalogSsmParameterRefs,
    credential_region: String,
}

fn durable_run_spec_preflight(spec: &RunSpec) -> Result<DurableRunSpecPreflight> {
    let resolved_nt_version =
        crate::nt_dependency_proof::verified_nt_revision_from_embedded_manifests()
            .context("resolve the workspace NautilusTrader revision for durable publication")?;
    ensure!(
        spec.manifest.resolved_nt_version == resolved_nt_version,
        "durable RunSpec NautilusTrader revision mismatch: configured {:?}, workspace {:?}",
        spec.manifest.resolved_nt_version,
        resolved_nt_version
    );
    ensure!(
        spec.source_proof.table_family == TRADE_TABLE_FAMILY,
        "durable operator currently supports only the proven {TRADE_TABLE_FAMILY:?} table family; got {:?}",
        spec.source_proof.table_family
    );
    let adapter =
        require_registered_source_adapter(&spec.converter.identity, &spec.converter.version)?;
    ensure!(
        adapter.kind == SourceAdapterKind::CsvNativeTrades,
        "durable operator currently supports only the proven CSV-native trade adapter; got {:?}",
        adapter.kind
    );
    let artifact_store = spec.required_artifact_store()?;
    spec.validate_artifact_store_publish_config(artifact_store)?;
    let artifact_root = artifact_store.resolve()?;
    let catalog_dispatch = spec.required_catalog_dispatch()?;
    catalog_dispatch.catalog_root_for(
        &spec.source_proof.source_binding,
        spec.manifest.market_structure_fixture,
        &artifact_root,
    )?;
    let credential_parameters = spec
        .manifest
        .artifact_store
        .ssm_parameters
        .as_ref()
        .context(
            "durable artifact-store publication requires manifest SSM credential parameters",
        )?;
    ensure!(
        credential_parameters.region == artifact_root.s3_region(),
        "durable artifact-store SSM region must match artifact-store S3 region"
    );
    Ok(DurableRunSpecPreflight {
        artifact_root,
        credential_parameters: NtCatalogSsmParameterRefs {
            access_key_id: credential_parameters.access_key_id.clone(),
            secret_access_key: credential_parameters.secret_access_key.clone(),
            session_token: credential_parameters.session_token.clone(),
        },
        credential_region: credential_parameters.region.clone(),
    })
}

/// Validate every durable-only RunSpec surface without reading source bytes,
/// resolving secrets, creating output, or contacting S3.
///
/// Callers must run the ordinary manifest/source-binding preflight first; that
/// consume boundary validates the configured SSM parameter-path syntax. This
/// durable boundary then validates store agreement, dispatch identity, region,
/// required SSM ownership, and the currently proven operator family.
pub fn validate_durable_run_spec_preflight(spec: &RunSpec) -> Result<()> {
    durable_run_spec_preflight(spec).map(|_| ())
}

fn durable_run_validation_context<'a>(
    spec: &'a RunSpec,
    versioning_enabled: &BucketVersioningEnabled,
    registry: &VerifiedSourceBindingRegistry,
) -> Result<(
    &'a CatalogDispatchConfig,
    ResolvedArtifactRoot,
    ConversionFingerprint,
)> {
    let artifact_store = spec.required_artifact_store()?;
    spec.validate_artifact_store_publish_config(artifact_store)?;
    let catalog_dispatch = spec.required_catalog_dispatch()?;
    let artifact_root = artifact_store.resolve()?;
    artifact_root.validate_bucket_versioning_capability(versioning_enabled)?;
    registry.reassert_for(spec)?;
    let (_expected_source_proof, accepted) = accepted_dataset_for_run_spec_hash_with_registry(
        spec,
        &spec.accepted_object.sha256,
        registry.registry(),
    )?;
    let fingerprint = conversion_fingerprint_for(spec, &accepted, registry)?;
    Ok((catalog_dispatch, artifact_root, fingerprint))
}

/// Prepared, budget-guarded durable dispatcher shared by every production
/// caller. Fields are private so an SSM credential or versioning proof cannot
/// be separated from the exact store it prepared.
pub(crate) struct DurableRunDispatcher {
    store: AmazonS3,
    versioning_enabled: BucketVersioningEnabled,
}

impl DurableRunDispatcher {
    /// Resolve the sole SSM credential source and establish the configured
    /// bucket-versioning capability after deterministic RunSpec preflight.
    pub(crate) async fn prepare_guarded(
        spec: &RunSpec,
        work_budget: &OperatorWorkBudgetGuard,
    ) -> Result<Self> {
        let preflight = guarded_operation_outcome(
            work_budget,
            OperatorWorkBudgetStage::ObjectVerification,
            || durable_run_spec_preflight(spec),
        )??;
        let resolver = guarded_async_operation_outcome(
            work_budget,
            OperatorWorkBudgetStage::Publish,
            NtCatalogSsmCredentialResolver::from_region(&preflight.credential_region),
        )
        .await??;
        let credentials = guarded_async_operation_outcome(
            work_budget,
            OperatorWorkBudgetStage::Publish,
            resolver.resolve(&preflight.credential_parameters),
        )
        .await??;
        let versioning_enabled = preflight
            .artifact_root
            .verify_bucket_versioning_enabled(&credentials, work_budget)
            .await?;
        let store = preflight
            .artifact_root
            .build_s3_object_store_with_credentials(&credentials)?;
        Ok(Self {
            store,
            versioning_enabled,
        })
    }

    /// Execute the sole durable operator under the caller's
    /// execution-plan-derived budget.
    pub(crate) async fn dispatch_guarded(
        &self,
        spec: &RunSpec,
        object_bytes: Vec<u8>,
        output_dir: &Path,
        registry: &VerifiedSourceBindingRegistry,
        work_budget: &OperatorWorkBudgetGuard,
    ) -> Result<DurableRunOutcome> {
        run_from_run_spec_with_verified_registry_guarded(
            spec,
            object_bytes,
            output_dir,
            &self.store,
            &self.versioning_enabled,
            registry,
            work_budget,
        )
        .await
    }

    /// Discover and fully validate the deterministic current terminal without
    /// consuming source bytes or entering any publication/BacktestNode path.
    pub(crate) async fn discover_current_completion_guarded(
        &self,
        spec: &RunSpec,
        registry: &VerifiedSourceBindingRegistry,
        work_budget: &OperatorWorkBudgetGuard,
    ) -> Result<Option<DurableRunReceipt>> {
        discover_current_durable_completion_with_artifact_store_guarded(
            spec,
            &self.store,
            &self.versioning_enabled,
            registry,
            work_budget,
        )
        .await
    }
}

pub(crate) async fn discover_current_durable_completion_with_artifact_store_guarded(
    spec: &RunSpec,
    store: &dyn ObjectStore,
    versioning_enabled: &BucketVersioningEnabled,
    registry: &VerifiedSourceBindingRegistry,
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<Option<DurableRunReceipt>> {
    let (catalog_dispatch, artifact_root, fingerprint) = guarded_operation_outcome(
        work_budget,
        OperatorWorkBudgetStage::ObjectVerification,
        || durable_run_validation_context(spec, versioning_enabled, registry),
    )??;
    let Some((locator, manifest_bytes)) =
        discover_current_durable_completion_guarded(store, &artifact_root, spec, work_budget)
            .await?
    else {
        return Ok(None);
    };
    let receipt = validate_durable_completion_manifest_bytes_guarded(
        DurableCompletionManifestValidation {
            spec,
            fingerprint: &fingerprint,
            artifact_root: &artifact_root,
            catalog_dispatch,
            store,
            work_budget,
        },
        &locator,
        manifest_bytes,
    )
    .await?;
    Ok(Some(receipt))
}

/// Test seam for exercising the durable write path without constructing the
/// production dispatcher. It is not compiled into the runtime library.
#[cfg(test)]
pub(crate) async fn run_from_run_spec_with_artifact_store_guarded(
    spec: &RunSpec,
    object_bytes: Vec<u8>,
    output_dir: &Path,
    store: &dyn ObjectStore,
    versioning_enabled: &BucketVersioningEnabled,
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<DurableRunOutcome> {
    let registry = VerifiedSourceBindingRegistry::from_run_spec_guarded(spec, work_budget)?;
    run_from_run_spec_with_verified_registry_guarded(
        spec,
        object_bytes,
        output_dir,
        store,
        versioning_enabled,
        &registry,
        work_budget,
    )
    .await
}

/// Guarded durable-catalog execution using a registry already pinned by the
/// caller. Exact-current terminal discovery is intentionally not selectable
/// through this write path.
async fn run_from_run_spec_with_verified_registry_guarded(
    spec: &RunSpec,
    object_bytes: Vec<u8>,
    output_dir: &Path,
    store: &dyn ObjectStore,
    versioning_enabled: &BucketVersioningEnabled,
    registry: &VerifiedSourceBindingRegistry,
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<DurableRunOutcome> {
    let (catalog_dispatch, artifact_root, fingerprint) = guarded_operation_outcome(
        work_budget,
        OperatorWorkBudgetStage::ObjectVerification,
        || durable_run_validation_context(spec, versioning_enabled, registry),
    )??;
    let source_binding_registry = registry.clone();

    let (base_spec, base_output_dir, base_work_budget) = guarded_operation_outcome(
        work_budget,
        OperatorWorkBudgetStage::ObjectVerification,
        || -> Result<_> { Ok((spec.clone(), output_dir.to_path_buf(), work_budget.clone())) },
    )??;
    let base_run = tokio::task::spawn_blocking(move || -> Result<_> {
        match prepare_run_from_run_spec_inner(
            &base_spec,
            &object_bytes,
            &base_output_dir,
            false,
            &source_binding_registry,
            &base_work_budget,
        )? {
            TradeRunPreparation::Prepared(prepared) => Ok(*prepared),
            TradeRunPreparation::Completed(_) => {
                bail!("durable preparation unexpectedly reused local completed output")
            }
        }
    });
    let prepared =
        guarded_blocking_join_outcome(work_budget, OperatorWorkBudgetStage::Publish, base_run)
            .await?
            .context("join durable preparation for artifact-store path")??;

    let writer = CreateOnlyArtifactWriter::new(store, &artifact_root);
    let expected_physical_manifest = match prepared
        .backtest
        .catalog_run_view_authority
        .roots
        .as_slice()
    {
        [root] => root.physical_manifest.clone(),
        roots => bail!(
            "single-table durable projection requires exactly one sealed catalog root, got {}",
            roots.len()
        ),
    };
    let persisted = match recover_catalog_projection_from_current_receipt_guarded(
        store,
        &artifact_root,
        versioning_enabled,
        catalog_dispatch,
        &spec.source_proof.source_binding,
        spec.manifest.market_structure_fixture,
        &expected_physical_manifest,
        work_budget,
    )
    .await?
    {
        Some(persisted) => persisted,
        None => {
            persist_catalog_projection_for_source_binding_guarded(
                store,
                &artifact_root,
                versioning_enabled,
                catalog_dispatch,
                &spec.source_proof.source_binding,
                spec.manifest.market_structure_fixture,
                &prepared.catalog_root,
                &expected_physical_manifest,
                work_budget,
            )
            .await?
        }
    };

    let hydration_root_lease = TransientCatalogRootLease::acquire(output_dir)
        .context("claim private exact-version hydration root")?;
    let hydrated = hydrate_catalog_projection_from_receipt_guarded(
        store,
        &artifact_root,
        &persisted.receipt_locator(),
        &expected_physical_manifest,
        &hydration_root_lease.catalog_root,
        work_budget,
    )
    .await?;
    ensure!(
        hydrated.catalog_root_uri == persisted.catalog_root_uri
            && hydrated.physical_manifest_sha256 == persisted.physical_manifest_sha256
            && hydrated.receipt_sha256 == persisted.receipt_sha256
            && hydrated.receipt_version_id == persisted.receipt_version_id,
        "hydrated catalog identity differs from the just-published immutable receipt"
    );
    let mut hydrated_manifest = prepared.local_manifest.clone();
    bind_runtime_manifest_to_local_catalog_root(
        &mut hydrated_manifest,
        hydrated.local_catalog_root(),
    )?;
    hydrated.revalidate_for_runner_seal_guarded(&expected_physical_manifest, work_budget)?;
    let (finalize_spec, finalize_output_dir, finalize_work_budget) = guarded_operation_outcome(
        work_budget,
        OperatorWorkBudgetStage::Backtest,
        || -> Result<_> { Ok((spec.clone(), output_dir.to_path_buf(), work_budget.clone())) },
    )??;
    let finalize_run = tokio::task::spawn_blocking(move || {
        finalize_prepared_trade_run(
            &finalize_spec,
            prepared,
            hydrated_manifest,
            &finalize_output_dir,
            false,
            &finalize_work_budget,
        )
    });
    let mut artifacts =
        guarded_blocking_join_outcome(work_budget, OperatorWorkBudgetStage::Backtest, finalize_run)
            .await?
            .context("join sole hydrated BacktestNode execution")??;
    let published_catalog_protocol = persisted
        .catalog_root_uri
        .split_once("://")
        .map(|(protocol, _)| protocol)
        .context("published catalog root URI is missing its protocol")?;
    let hydrated_catalog_proof = PublishedCatalogProof {
        proof_version: PUBLISHED_CATALOG_PROOF_VERSION.to_string(),
        catalog_uri: persisted.catalog_root_uri.clone(),
        catalog_fs_protocol: published_catalog_protocol.to_string(),
        publication_receipt_uri: persisted.receipt_uri.clone(),
        publication_receipt_sha256: persisted.receipt_sha256.clone(),
        publication_receipt_version_id: persisted.receipt_version_id.clone(),
        publication_physical_manifest_sha256: persisted.physical_manifest_sha256.clone(),
        expected_iterations: artifacts.output.expected_iterations,
        nt_iterations: artifacts.output.nt_result.iterations,
        run_config_id: artifacts.output.nt_result.run_config_id.clone(),
        nt_version: artifacts.output.contract.nt_version.clone(),
        created_at: spec.created_at_utc.clone(),
    };

    guarded_operation_outcome(
        work_budget,
        OperatorWorkBudgetStage::Publish,
        || -> Result<()> {
            artifacts.output.conversion_catalog_metadata = artifacts
                .output
                .conversion_catalog_metadata
                .clone()
                .with_catalog_consumption_evidence(
                    CatalogConsumptionEvidence::HydratedPublication {
                        local_catalog_root: hydrated.local_catalog_root().to_path_buf(),
                        receipt: CatalogPublicationReceiptIdentity {
                            catalog_root_uri: persisted.catalog_root_uri.clone(),
                            receipt_uri: persisted.receipt_uri.clone(),
                            receipt_sha256: persisted.receipt_sha256.clone(),
                            receipt_version_id: persisted.receipt_version_id.clone(),
                            physical_manifest_sha256: persisted.physical_manifest_sha256.clone(),
                        },
                    },
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
            artifacts.output.contract.artifact_uris.nt_catalog_uri =
                persisted.catalog_root_uri.clone();
            replace_contract_claim_limit_uri(
                &mut artifacts.output.contract,
                &transient_catalog_uri,
                &persisted.catalog_root_uri,
            );
            artifacts
                .output
                .contract
                .artifact_uris
                .nt_catalog_manifest_uri = Some(persisted.receipt_uri.clone());
            artifacts.output.contract.validate().map_err(|error| {
                anyhow::anyhow!("durable result contract validation failed: {error}")
            })?;
            hydrated_catalog_proof.validate_against(
                &artifacts.output.conversion_catalog_metadata,
                &artifacts.output.contract,
                spec,
            )?;
            let proof_bytes = serde_json::to_vec_pretty(&hydrated_catalog_proof)
                .context("serialize hydrated catalog proof")?;
            persist_immutable_local_bytes_guarded(
                &output_dir.join(PUBLISHED_CATALOG_PROOF_FILE),
                &proof_bytes,
                "published catalog proof",
                work_budget,
                OperatorWorkBudgetStage::Publish,
            )?;
            write_pending_conversion_artifacts(
                output_dir,
                &artifacts.output.conversion_manifest,
                &artifacts.output.conversion_catalog_metadata,
                work_budget,
            )?;
            let contract_bytes =
                crate::reference_artifact::canonical_json_bytes(&artifacts.output.contract)
                    .context("serialize durable result contract")?;
            persist_immutable_local_bytes_guarded(
                &artifacts.contract_path,
                &contract_bytes,
                "durable result contract",
                work_budget,
                OperatorWorkBudgetStage::Publish,
            )
            .with_context(|| format!("write {}", artifacts.contract_path.display()))?;
            Ok(())
        },
    )??;
    let durable_artifacts = persist_durable_contract_artifacts(
        &writer,
        &artifact_root,
        &artifacts,
        &spec.manifest.output_prefix,
        work_budget,
    )
    .await?;
    let publication_receipt = DurableObjectVersionIdentity {
        uri: persisted.receipt_uri.clone(),
        sha256: persisted.receipt_sha256.clone(),
        byte_len: persisted.receipt_byte_len,
        version_id: persisted.receipt_version_id.clone(),
        e_tag: persisted.receipt_e_tag.clone(),
    };
    let completion_manifest = DurableCompletionManifest::new(
        spec,
        fingerprint.clone(),
        &artifacts.batch_summary,
        publication_receipt,
        durable_artifacts,
    );
    validate_durable_result_contract_cross_claims(
        &artifacts.output.contract,
        spec,
        &fingerprint,
        &completion_manifest,
    )
    .context("cross-validate fresh durable result contract before terminal create")?;
    let completion_payload = crate::reference_artifact::canonical_json_bytes(&completion_manifest)
        .context("serialize durable completion manifest")?;
    let completion_sha256 = sha256_hex_with_budget(
        &completion_payload,
        work_budget,
        OperatorWorkBudgetStage::Publish,
    )?;
    let completion_byte_len = u64::try_from(completion_payload.len())
        .context("durable completion manifest length does not fit u64")?;
    ensure!(
        completion_byte_len <= artifact_root.max_final_object_bytes(),
        "durable completion manifest exceeds artifact_store.max_final_object_bytes"
    );
    let completion_uri = portable_artifact_uri(
        &spec.manifest.output_prefix,
        DURABLE_COMPLETION_MANIFEST_FILE,
    );
    let prepared_completion = writer.prepare_terminal_create_uri(
        &artifact_root,
        &completion_uri,
        completion_payload,
        format!("durable completion manifest {completion_uri}"),
    )?;
    artifacts.canonical_catalog_uri = Some(persisted.catalog_root_uri.clone());
    artifacts.persisted_catalog_objects = persisted.objects.clone();
    artifacts.persisted_catalog_projection = Some(persisted);
    drop(hydrated);
    hydration_root_lease.finish_retained(work_budget)?;
    let transient_catalog_root_lease = artifacts
        .transient_catalog_root_lease
        .take()
        .context("durable operator is missing its transient catalog-root ownership lease")?;
    transient_catalog_root_lease.finish_retained(work_budget)?;
    // Every local byte and retained directory is final at this boundary. The
    // create-only candidate is intentionally written before the remote
    // terminal attempt, and is never itself a completion authority.
    commit_durable_operator_output_candidate(
        spec,
        &fingerprint,
        output_dir,
        &artifacts.batch_summary,
        work_budget,
    )?;
    let permit = work_budget.authorize_commit(OperatorWorkBudgetStage::Publish)?;
    let confirmed_completion = writer
        .create_or_confirm_terminal(prepared_completion, permit)
        .await?;
    let completion = DurableCompletionLocator {
        object: DurableObjectVersionIdentity {
            uri: completion_uri,
            sha256: completion_sha256,
            byte_len: completion_byte_len,
            version_id: confirmed_completion.version_id,
            e_tag: confirmed_completion.e_tag,
        },
    };
    let receipt = DurableRunReceipt {
        completion,
        run_id: completion_manifest.run_id,
        submitted_manifest_hash: completion_manifest.submitted_manifest_hash,
        canonical_rows: completion_manifest.canonical_rows,
        nt_catalog_rows: completion_manifest.nt_catalog_rows,
        catalog_hash: completion_manifest.catalog_hash,
    };
    // The remote terminal manifest is the only durable completion authority.
    // No I/O or fallible cleanup follows its create-only PUT.
    Ok(DurableRunOutcome {
        #[cfg(test)]
        artifacts: Box::new(artifacts),
        receipt,
    })
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
    fn ts_init_range(&self, work_budget: &OperatorWorkBudgetGuard) -> Result<Option<(u64, u64)>> {
        fn fold<R>(
            rows: &[R],
            work_budget: &OperatorWorkBudgetGuard,
            row_materialized_bytes: impl Fn(&R) -> Result<usize>,
            ts_init: impl Fn(&R) -> Result<u64>,
        ) -> Result<Option<(u64, u64)>> {
            let mut range: Option<(u64, u64)> = None;
            verify_canonical_rows_materialization(
                rows,
                work_budget,
                OperatorWorkBudgetStage::Backtest,
                row_materialized_bytes,
            )?;
            for row in rows {
                work_budget.check_deadline(OperatorWorkBudgetStage::Backtest)?;
                let ts = ts_init(row)?;
                range = Some(match range {
                    Some((min, max)) => (min.min(ts), max.max(ts)),
                    None => (ts, ts),
                });
                work_budget.check_deadline(OperatorWorkBudgetStage::Backtest)?;
            }
            Ok(range)
        }
        match self {
            Self::Trades(table) => fold(
                &table.rows,
                work_budget,
                canonical_trade_row_materialized_bytes,
                |row| {
                    Ok(ts_init_nanos(
                        row.availability_time,
                        row.capture_time,
                        &format!("trade {}", row.trade_id),
                    )?
                    .as_u64())
                },
            ),
            Self::Bars(table) => {
                let bar_aggregation = table.bar_spec.aggregation.to_string();
                fold(
                    &table.rows,
                    work_budget,
                    |row| bar_row_materialized_bytes(row, &bar_aggregation),
                    |row| {
                        Ok(ts_init_nanos(
                            row.availability_time,
                            row.capture_time,
                            &format!("bar close_time {}", row.close_time),
                        )?
                        .as_u64())
                    },
                )
            }
            Self::Deltas(table) => fold(
                &table.rows,
                work_budget,
                delta_row_materialized_bytes,
                |row| {
                    Ok(ts_init_nanos(
                        row.availability_time,
                        row.capture_time,
                        &format!("delta sequence {}", row.sequence),
                    )?
                    .as_u64())
                },
            ),
            Self::Quotes(table) => fold(
                &table.rows,
                work_budget,
                quote_row_materialized_bytes,
                |row| {
                    Ok(ts_init_nanos(
                        row.availability_time,
                        row.capture_time,
                        &format!("quote {}", row.event_time),
                    )?
                    .as_u64())
                },
            ),
            Self::Index(table) => fold(
                &table.rows,
                work_budget,
                point_price_row_materialized_bytes,
                |row| {
                    Ok(ts_init_nanos(
                        row.availability_time,
                        row.capture_time,
                        &format!("index price {}", row.event_time),
                    )?
                    .as_u64())
                },
            ),
            Self::Mark(table) => fold(
                &table.rows,
                work_budget,
                mark_price_row_materialized_bytes,
                |row| {
                    Ok(ts_init_nanos(
                        row.availability_time,
                        row.capture_time,
                        &format!("mark price {}", row.event_time),
                    )?
                    .as_u64())
                },
            ),
            Self::Funding(table) => fold(
                &table.rows,
                work_budget,
                funding_rate_row_materialized_bytes,
                |row| {
                    Ok(ts_init_nanos(
                        row.availability_time,
                        row.capture_time,
                        &format!("funding rate {}", row.event_time),
                    )?
                    .as_u64())
                },
            ),
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
    fn windowed_count(
        &self,
        start: Option<u64>,
        end: Option<u64>,
        work_budget: &OperatorWorkBudgetGuard,
    ) -> Result<usize> {
        fn count<R>(
            rows: &[R],
            start: Option<u64>,
            end: Option<u64>,
            work_budget: &OperatorWorkBudgetGuard,
            row_materialized_bytes: impl Fn(&R) -> Result<usize>,
            ts_init: impl Fn(&R) -> Result<u64>,
        ) -> Result<usize> {
            let mut total = 0usize;
            verify_canonical_rows_materialization(
                rows,
                work_budget,
                OperatorWorkBudgetStage::Backtest,
                row_materialized_bytes,
            )?;
            for row in rows {
                work_budget.check_deadline(OperatorWorkBudgetStage::Backtest)?;
                let ts = ts_init(row)?;
                if start.is_none_or(|start| ts >= start) && end.is_none_or(|end| ts <= end) {
                    total = total
                        .checked_add(1)
                        .context("expected iteration count overflow")?;
                }
                work_budget.check_deadline(OperatorWorkBudgetStage::Backtest)?;
            }
            Ok(total)
        }
        match self {
            Self::Trades(table) => count(
                &table.rows,
                start,
                end,
                work_budget,
                canonical_trade_row_materialized_bytes,
                |row| {
                    Ok(ts_init_nanos(
                        row.availability_time,
                        row.capture_time,
                        &format!("trade {}", row.trade_id),
                    )?
                    .as_u64())
                },
            ),
            Self::Bars(table) => {
                let bar_aggregation = table.bar_spec.aggregation.to_string();
                count(
                    &table.rows,
                    start,
                    end,
                    work_budget,
                    |row| bar_row_materialized_bytes(row, &bar_aggregation),
                    |row| {
                        Ok(ts_init_nanos(
                            row.availability_time,
                            row.capture_time,
                            &format!("bar close_time {}", row.close_time),
                        )?
                        .as_u64())
                    },
                )
            }
            Self::Deltas(table) => count(
                &table.rows,
                start,
                end,
                work_budget,
                delta_row_materialized_bytes,
                |row| {
                    Ok(ts_init_nanos(
                        row.availability_time,
                        row.capture_time,
                        &format!("delta sequence {}", row.sequence),
                    )?
                    .as_u64())
                },
            ),
            Self::Quotes(table) => count(
                &table.rows,
                start,
                end,
                work_budget,
                quote_row_materialized_bytes,
                |row| {
                    Ok(ts_init_nanos(
                        row.availability_time,
                        row.capture_time,
                        &format!("quote {}", row.event_time),
                    )?
                    .as_u64())
                },
            ),
            Self::Index(table) => count(
                &table.rows,
                start,
                end,
                work_budget,
                point_price_row_materialized_bytes,
                |row| {
                    Ok(ts_init_nanos(
                        row.availability_time,
                        row.capture_time,
                        &format!("index price {}", row.event_time),
                    )?
                    .as_u64())
                },
            ),
            Self::Mark(table) => count(
                &table.rows,
                start,
                end,
                work_budget,
                mark_price_row_materialized_bytes,
                |row| {
                    Ok(ts_init_nanos(
                        row.availability_time,
                        row.capture_time,
                        &format!("mark price {}", row.event_time),
                    )?
                    .as_u64())
                },
            ),
            Self::Funding(table) => count(
                &table.rows,
                start,
                end,
                work_budget,
                funding_rate_row_materialized_bytes,
                |row| {
                    Ok(ts_init_nanos(
                        row.availability_time,
                        row.capture_time,
                        &format!("funding rate {}", row.event_time),
                    )?
                    .as_u64())
                },
            ),
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
    pub catalog_run_view_authority_path: PathBuf,
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
    pub(crate) batch_summary: OperatorRunSummary,
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
    let local_spec = LocalRunSpec::new(spec)?;
    let registry = VerifiedSourceBindingRegistry::from_run_spec(spec)?;
    run_operator_from_local_run_spec_with_verified_registry(
        local_spec,
        object_bytes,
        output_dir,
        &registry,
        &OperatorWorkBudgetGuard::unbounded(),
    )
}

pub fn run_operator_from_run_spec_guarded(
    spec: &RunSpec,
    object_bytes: &[u8],
    output_dir: &Path,
    registry: &VerifiedSourceBindingRegistry,
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<OperatorRunArtifacts> {
    run_operator_from_local_run_spec_with_verified_registry(
        LocalRunSpec::new(spec)?,
        object_bytes,
        output_dir,
        registry,
        work_budget,
    )
}

fn run_operator_from_local_run_spec_with_verified_registry(
    local_spec: LocalRunSpec<'_>,
    object_bytes: &[u8],
    output_dir: &Path,
    registry: &VerifiedSourceBindingRegistry,
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<OperatorRunArtifacts> {
    let spec = local_spec.get();
    registry.reassert_for(spec)?;
    let adapter =
        require_registered_source_adapter(&spec.converter.identity, &spec.converter.version)?;
    if adapter.kind == SourceAdapterKind::CsvNativeTrades {
        return Ok(OperatorRunArtifacts::Trade(Box::new(
            run_from_local_run_spec_with_verified_registry(
                local_spec,
                object_bytes,
                output_dir,
                registry,
                work_budget,
            )?,
        )));
    }
    Ok(OperatorRunArtifacts::MultiTable(Box::new(
        run_multi_table_from_run_spec_with_verified_registry(
            local_spec,
            object_bytes,
            output_dir,
            registry,
            work_budget,
        )?,
    )))
}

/// Explicit unit-test seam used only by `LocalSourceUniverseOperatorRunner`.
/// Durable RunSpecs may enter this seam so the test runner can exercise local
/// conversion before replacing its terminal seal with a candidate seal. The
/// production library has no corresponding alternate entry point.
#[cfg(test)]
pub(crate) fn run_operator_from_run_spec_with_verified_registry(
    spec: &RunSpec,
    object_bytes: &[u8],
    output_dir: &Path,
    registry: &VerifiedSourceBindingRegistry,
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<OperatorRunArtifacts> {
    run_operator_from_local_run_spec_with_verified_registry(
        LocalRunSpec::for_source_universe_test(spec),
        object_bytes,
        output_dir,
        registry,
        work_budget,
    )
}

fn conversion_fingerprint_for(
    spec: &RunSpec,
    accepted: &AcceptedDataset,
    registry: &VerifiedSourceBindingRegistry,
) -> Result<ConversionFingerprint> {
    registry.reassert_for(spec)?;
    let fingerprint = ConversionFingerprint {
        source_proof_id: accepted.source_proof_id.clone(),
        source_proof_version: accepted.source_proof_version,
        accepted_object_sha256: accepted.accepted_object_sha256.clone(),
        control_artifact_path: spec
            .source_bindings_path
            .to_str()
            .context("source_bindings_path is not valid UTF-8")?
            .to_string(),
        control_artifact_sha256: registry.sha256().to_string(),
        converter_identity: spec.converter.identity.clone(),
        converter_version: spec.converter.version.clone(),
        converter_config_hash: spec
            .converter
            .content_hash()
            .context("hash converter config")?,
    };
    fingerprint.validate()?;
    Ok(fingerprint)
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
                bytes,
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
fn assert_planned_read_back(
    planned: &PlannedTable,
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<()> {
    match &planned.table {
        NormalizedTable::Trades(table) => {
            let ticks = read_back_trade_ticks_guarded(
                &planned.subroot,
                &planned.nt_instrument_id,
                work_budget,
            )
            .context("catalog read-back failed")?;
            ensure!(
                ticks.len() == table.rows.len(),
                "catalog read-back {} does not match projected {} trades",
                ticks.len(),
                table.rows.len()
            );
            assert_read_back_matches_guarded(
                &ticks,
                &table.rows,
                &planned.nt_instrument_id,
                work_budget,
            )
        }
        NormalizedTable::Bars(table) => {
            let bars =
                read_back_bars_guarded(&planned.subroot, &planned.nt_instrument_id, work_budget)
                    .context("catalog read-back failed")?;
            assert_bar_read_back_matches_guarded(
                &bars,
                table,
                &planned.nt_instrument_id,
                work_budget,
            )
        }
        NormalizedTable::Deltas(table) => {
            let deltas = read_back_order_book_deltas_guarded(
                &planned.subroot,
                &planned.nt_instrument_id,
                work_budget,
            )
            .context("catalog read-back failed")?;
            assert_delta_read_back_matches_guarded(
                &deltas,
                table,
                &planned.nt_instrument_id,
                work_budget,
            )
        }
        NormalizedTable::Quotes(table) => {
            let quotes =
                read_back_quotes_guarded(&planned.subroot, &planned.nt_instrument_id, work_budget)
                    .context("catalog read-back failed")?;
            assert_quote_read_back_matches_guarded(
                &quotes,
                table,
                &planned.nt_instrument_id,
                work_budget,
            )
        }
        NormalizedTable::Index(table) => {
            let prices =
                read_back_index_guarded(&planned.subroot, &planned.nt_instrument_id, work_budget)
                    .context("catalog read-back failed")?;
            assert_index_read_back_matches_guarded(
                &prices,
                table,
                &planned.nt_instrument_id,
                work_budget,
            )
        }
        NormalizedTable::Mark(table) => {
            let prices =
                read_back_mark_guarded(&planned.subroot, &planned.nt_instrument_id, work_budget)
                    .context("catalog read-back failed")?;
            assert_mark_read_back_matches_guarded(
                &prices,
                table,
                &planned.nt_instrument_id,
                work_budget,
            )
        }
        NormalizedTable::Funding(table) => {
            let rates = read_back_funding_rates_guarded(
                &planned.subroot,
                &planned.nt_instrument_id,
                work_budget,
            )
            .context("catalog read-back failed")?;
            assert_funding_read_back_matches_guarded(
                &rates,
                table,
                &planned.nt_instrument_id,
                work_budget,
            )
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

fn bind_completed_catalog_inputs(
    spec: &RunSpec,
    output_dir: &Path,
    records: &[ConversionTableRecord],
) -> Result<BacktestingRunManifest> {
    let mut manifest = spec.manifest.clone();
    let mut used = vec![false; records.len()];
    for input in &mut manifest.catalog_inputs {
        let candidates = records
            .iter()
            .enumerate()
            .filter(|(index, record)| {
                !used[*index]
                    && record.nt_instrument_id == input.nt_instrument_id
                    && record.data_type == input.data_type
                    && match (&input.bar_spec, &record.bar_spec) {
                        (Some(expected), Some(actual)) => expected == actual,
                        (None, _) => true,
                        (Some(_), None) => false,
                    }
            })
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        let index = match candidates.as_slice() {
            [index] => *index,
            [] => bail!(
                "completed catalog input {}/{} (bar_spec {:?}) matches no conversion-table record",
                input.nt_instrument_id,
                input.data_type,
                input.bar_spec
            ),
            _ => bail!(
                "completed catalog input {}/{} (bar_spec {:?}) ambiguously matches {} conversion-table records",
                input.nt_instrument_id,
                input.data_type,
                input.bar_spec,
                candidates.len()
            ),
        };
        used[index] = true;
        input.catalog_path = output_dir
            .join(&records[index].subroot_uri)
            .to_str()
            .context("completed catalog subroot path is not UTF-8")?
            .to_string();
        input.catalog_fs_protocol = CATALOG_FS_PROTOCOL_NONE.to_string();
        input.catalog_fs_storage_options.clear();
        input.catalog_fs_rust_storage_options.clear();
    }
    ensure!(
        used.into_iter().all(|used| used),
        "completed conversion-table records contain an unbound catalog root"
    );
    Ok(manifest)
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
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<()> {
    let start = window_bound_nanos("start_time", manifest.start_time)?;
    let end = window_bound_nanos("end_time", manifest.end_time)?;
    for table in planned {
        let Some((first, last)) = table.table.ts_init_range(work_budget)? else {
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
    let local_spec = LocalRunSpec::new(spec)?;
    let registry = VerifiedSourceBindingRegistry::from_run_spec(spec)?;
    run_multi_table_from_run_spec_with_verified_registry(
        local_spec,
        object_bytes,
        output_dir,
        &registry,
        &OperatorWorkBudgetGuard::unbounded(),
    )
}

fn run_multi_table_from_run_spec_with_verified_registry(
    local_spec: LocalRunSpec<'_>,
    object_bytes: &[u8],
    output_dir: &Path,
    registry: &VerifiedSourceBindingRegistry,
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<MultiTableRunArtifacts> {
    let spec = local_spec.get();
    registry.reassert_for(spec)?;
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

    let verified_sha256 = sha256_hex_with_budget(
        object_bytes,
        work_budget,
        OperatorWorkBudgetStage::ObjectVerification,
    )?;
    ensure!(
        verified_sha256 == spec.accepted_object.sha256,
        "object SHA-256 {verified_sha256} does not match run-spec {}",
        spec.accepted_object.sha256
    );

    // Gate 1: accept the source proof and bind the object via the ledger.
    let (accepted_proof, accepted) = accepted_dataset_for_run_spec_hash_with_registry(
        spec,
        &verified_sha256,
        registry.registry(),
    )?;
    validate_converter_table_family(&spec.converter, &accepted.table_family)?;
    // Gate 4 preflight on the declared (placeholder-path) inputs, before any
    // artifact is produced.
    validate_local_run_manifest(&spec.manifest, &accepted)?;

    let conversion_fingerprint = conversion_fingerprint_for(spec, &accepted, registry)?;
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

    let sealed_completion = preflight_completed_output_before_inspection(
        spec,
        &accepted_proof,
        &accepted,
        output_dir,
        &conversion_fingerprint,
        work_budget,
    )?;
    let completed = match inspect_conversion_output(output_dir, &conversion_fingerprint)? {
        ConversionOutputState::Complete {
            manifest_hash,
            checkpoint_hash,
            catalog_hash,
        } if sealed_completion => Some((manifest_hash, checkpoint_hash, catalog_hash)),
        ConversionOutputState::Complete { .. }
        | ConversionOutputState::CleanNew
        | ConversionOutputState::ResumeFromCheckpoint { .. } => None,
    };

    // Decode and normalize on both paths: the completed path re-derives the
    // canonical tables in memory to re-prove read-back equality and the
    // engine-iteration expectation without re-projecting verified subroots.
    work_budget.check_deadline(OperatorWorkBudgetStage::Decode)?;
    let payload = decode_object_payload(&spec.converter.raw_payload, object_bytes, work_budget)?;
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
        let completed_inputs = MultiCompletedInputs {
            spec,
            accepted: &accepted,
            accepted_proof,
            conversion_fingerprint: &conversion_fingerprint,
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
        };
        return run_budgeted_stage(work_budget, OperatorWorkBudgetStage::Finalize, || {
            run_multi_from_completed_output(completed_inputs)
        });
    }

    work_budget.check_deadline(OperatorWorkBudgetStage::CanonicalWrite)?;
    fs::create_dir_all(output_dir)
        .with_context(|| format!("create output dir {}", output_dir.display()))?;
    // Gates 2+3 per table: projection, read-back, equality, canonical artifact.
    let mut catalog_hashes = Vec::with_capacity(planned.len());
    for table in &planned {
        work_budget.check_deadline(OperatorWorkBudgetStage::CatalogProjection)?;
        let instrument_spec = resolve_instrument_spec(&spec.instrument_spec, table, table_count)?;
        let projection = match &table.table {
            NormalizedTable::Trades(canonical) => project_canonical_trades_to_catalog_guarded(
                canonical,
                instrument_spec,
                &table.subroot,
                output_dir,
                work_budget,
            ),
            NormalizedTable::Bars(canonical) => project_canonical_bars_to_catalog_guarded(
                canonical,
                instrument_spec,
                &table.subroot,
                output_dir,
                work_budget,
            ),
            NormalizedTable::Deltas(canonical) => {
                project_canonical_order_book_deltas_to_catalog_guarded(
                    canonical,
                    instrument_spec,
                    &table.subroot,
                    output_dir,
                    work_budget,
                )
            }
            NormalizedTable::Quotes(canonical) => project_canonical_quotes_to_catalog_guarded(
                canonical,
                instrument_spec,
                &table.subroot,
                output_dir,
                work_budget,
            ),
            NormalizedTable::Index(canonical) => project_canonical_index_to_catalog_guarded(
                canonical,
                instrument_spec,
                &table.subroot,
                output_dir,
                work_budget,
            ),
            NormalizedTable::Mark(canonical) => project_canonical_mark_to_catalog_guarded(
                canonical,
                instrument_spec,
                &table.subroot,
                output_dir,
                work_budget,
            ),
            NormalizedTable::Funding(canonical) => {
                project_canonical_funding_rates_to_catalog_guarded(
                    canonical,
                    instrument_spec,
                    &table.subroot,
                    output_dir,
                    work_budget,
                )
            }
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
        assert_planned_read_back(table, work_budget)?;
        let parent = table
            .canonical_path
            .parent()
            .context("canonical artifact path has no parent")?;
        fs::create_dir_all(parent)
            .with_context(|| format!("create canonical artifact dir {}", parent.display()))?;
        work_budget.check_deadline(OperatorWorkBudgetStage::CanonicalWrite)?;
        match &table.table {
            NormalizedTable::Trades(canonical) => {
                canonical.write_parquet_guarded(&table.canonical_path, work_budget)
            }
            NormalizedTable::Bars(canonical) => {
                canonical.write_parquet_guarded(&table.canonical_path, work_budget)
            }
            NormalizedTable::Deltas(canonical) => {
                canonical.write_parquet_guarded(&table.canonical_path, work_budget)
            }
            NormalizedTable::Quotes(canonical) => {
                canonical.write_parquet_guarded(&table.canonical_path, work_budget)
            }
            NormalizedTable::Index(canonical) => {
                canonical.write_parquet_guarded(&table.canonical_path, work_budget)
            }
            NormalizedTable::Mark(canonical) => {
                canonical.write_parquet_guarded(&table.canonical_path, work_budget)
            }
            NormalizedTable::Funding(canonical) => {
                canonical.write_parquet_guarded(&table.canonical_path, work_budget)
            }
        }
        .with_context(|| {
            format!(
                "write canonical artifact {}",
                table.canonical_path.display()
            )
        })?;
        catalog_hashes.push(projection.catalog_hash);
    }

    let (actual_rows, actual_row_groups) = planned.iter().try_fold(
        (0_u64, 0_u64),
        |(rows, row_groups), table| -> Result<(u64, u64)> {
            let metadata = actual_nt_market_data_metadata_guarded(&table.subroot, work_budget)?;
            Ok((
                rows.checked_add(metadata.rows)
                    .context("actual projected row total overflow")?,
                row_groups
                    .checked_add(metadata.row_groups)
                    .context("actual projected row-group total overflow")?,
            ))
        },
    )?;
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
    assert_tables_overlap_window(&local_manifest, &planned, work_budget)?;
    let primary_index = *bound_indices
        .first()
        .context("manifest must declare at least one catalog input")?;
    let primary = &planned[primary_index];
    let primary_catalog_hash = catalog_hashes[primary_index].clone();
    let artifact_uris = multi_artifact_uris(&spec.manifest, primary);
    let (event_count_ledger_hash, selected_asset_ids_hash) =
        multi_selector_provenance(spec, &planned)?;

    let submitted_identity = submitted_run_identity_for_spec(spec)?;
    let mut logical_catalog_hashes = Vec::new();
    logical_catalog_hashes
        .try_reserve_exact(bound_indices.len())
        .context("reserve bound logical catalog hashes")?;
    for index in &bound_indices {
        logical_catalog_hashes.push(
            catalog_hashes
                .get(*index)
                .context("bound catalog hash index left projected bounds")?
                .clone(),
        );
    }
    let catalog_run_view_authority = mint_local_catalog_run_view_authority_guarded(
        &local_manifest,
        &submitted_identity,
        &logical_catalog_hashes,
        work_budget,
    )?;
    persist_catalog_run_view_authority_guarded(
        spec,
        &local_manifest,
        &catalog_run_view_authority,
        output_dir,
        work_budget,
    )?;

    // Gate 5: ONE BacktestNode run over the N-input manifest.
    let nt_run = run_nt_backtest_node_guarded(
        &local_manifest,
        &submitted_identity,
        &catalog_run_view_authority,
        work_budget,
    )?;
    work_budget.check_deadline(OperatorWorkBudgetStage::Backtest)?;
    let nt_result = nt_run.result;
    let config_override_report = nt_run.config_override_report;
    let run_guard_report = nt_run.run_guard_report;
    let window_start = window_bound_nanos("start_time", local_manifest.start_time)?;
    let window_end = window_bound_nanos("end_time", local_manifest.end_time)?;
    let mut expected = 0usize;
    for table in &planned {
        work_budget.check_deadline(OperatorWorkBudgetStage::Backtest)?;
        let table_count = table
            .table
            .windowed_count(window_start, window_end, work_budget)
            .context("compute expected engine iterations for projected table")?;
        expected = expected
            .checked_add(table_count)
            .context("aggregate expected engine iteration count overflow")?;
        work_budget.check_deadline(OperatorWorkBudgetStage::Backtest)?;
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
        conversion_fingerprint.clone(),
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

    let proof_bytes =
        serde_json::to_vec_pretty(&accepted_proof).context("serialize accepted source proof")?;
    persist_immutable_local_bytes_guarded(
        &proof_path,
        &proof_bytes,
        "accepted source proof",
        work_budget,
        OperatorWorkBudgetStage::Finalize,
    )?;
    let contract_bytes =
        serde_json::to_vec_pretty(&contract).context("serialize result contract")?;
    persist_immutable_local_bytes_guarded(
        &contract_path,
        &contract_bytes,
        "result contract",
        work_budget,
        OperatorWorkBudgetStage::Finalize,
    )?;
    let run_manifest_bytes = serde_json::to_vec_pretty(&spec.manifest.to_artifact_manifest()?)
        .context("serialize resolved run manifest")?;
    persist_immutable_local_bytes_guarded(
        &run_manifest_path,
        &run_manifest_bytes,
        "resolved run manifest",
        work_budget,
        OperatorWorkBudgetStage::Finalize,
    )?;
    let conversion_tables_path = if planned.len() > 1 {
        let records: Vec<ConversionTableRecord> = planned
            .iter()
            .zip(catalog_hashes.iter())
            .map(|(table, hash)| table.record(hash.clone()))
            .collect();
        Some(write_conversion_tables_index_guarded(
            output_dir,
            &records,
            work_budget,
        )?)
    } else {
        None
    };
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
        .collect::<Vec<_>>();
    let batch_summary = OperatorRunSummary::multi(&tables, &conversion_manifest.catalog_hash)?;
    let artifacts = MultiTableRunArtifacts {
        verified_sha256,
        accepted_source_proof: accepted_proof,
        proof_path,
        contract_path,
        run_manifest_path,
        conversion_manifest_path,
        conversion_checkpoint_path,
        catalog_metadata_path,
        catalog_run_view_authority_path: output_dir.join(CATALOG_RUN_VIEW_AUTHORITY_FILE),
        conversion_tables_path,
        tables,
        conversion_checkpoint,
        conversion_manifest,
        conversion_catalog_metadata,
        conversion_checkpoint_hash,
        conversion_manifest_hash,
        nt_result,
        contract,
        batch_summary,
    };
    write_completed_conversion_artifacts_guarded(
        output_dir,
        &artifacts.conversion_manifest,
        &artifacts.conversion_checkpoint,
        &artifacts.conversion_catalog_metadata,
        work_budget,
    )?;
    commit_operator_terminal_seal(
        spec,
        &artifacts.accepted_source_proof,
        &accepted,
        &conversion_fingerprint,
        output_dir,
        &artifacts.batch_summary,
        work_budget,
    )?;
    Ok(artifacts)
}

struct MultiCompletedInputs<'a> {
    spec: &'a RunSpec,
    accepted: &'a AcceptedDataset,
    accepted_proof: SourceProofReport,
    conversion_fingerprint: &'a ConversionFingerprint,
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
    let seal: OperatorTerminalSeal = read_json_artifact_guarded(
        &inputs.output_dir.join(OPERATOR_TERMINAL_SEAL_FILE),
        inputs.work_budget,
        OperatorWorkBudgetStage::Finalize,
    )?;
    let current_files =
        collect_operator_terminal_seal_files(inputs.output_dir, inputs.work_budget)?;
    let sealed_summary =
        verify_completed_operator_output_against_seal(CompletedOperatorOutputVerification {
            spec,
            expected_source_proof: &inputs.accepted_proof,
            accepted,
            fingerprint: inputs.conversion_fingerprint,
            output_dir: inputs.output_dir,
            seal: &seal,
            current_files: &current_files,
            verify_physical_catalog_view: false,
            work_budget: inputs.work_budget,
        })?;
    let planned = inputs.planned;

    inputs
        .work_budget
        .check_deadline(OperatorWorkBudgetStage::CatalogProjection)?;
    let conversion_checkpoint: ConversionCheckpoint = read_json_artifact_guarded(
        &inputs.conversion_checkpoint_path,
        inputs.work_budget,
        OperatorWorkBudgetStage::CatalogProjection,
    )?;
    ensure!(
        conversion_checkpoint.content_hash()? == inputs.conversion_checkpoint_hash,
        "completed conversion checkpoint hash changed after inspection"
    );
    inputs
        .work_budget
        .check_deadline(OperatorWorkBudgetStage::CatalogProjection)?;
    let conversion_manifest: ConversionManifest = read_json_artifact_guarded(
        &inputs.conversion_manifest_path,
        inputs.work_budget,
        OperatorWorkBudgetStage::CatalogProjection,
    )?;
    ensure!(
        conversion_manifest.content_hash()? == inputs.conversion_manifest_hash,
        "completed conversion manifest hash changed after inspection"
    );
    inputs
        .work_budget
        .check_deadline(OperatorWorkBudgetStage::CatalogProjection)?;
    let conversion_catalog_metadata: ConversionCatalogMetadata = read_json_artifact_guarded(
        &inputs.catalog_metadata_path,
        inputs.work_budget,
        OperatorWorkBudgetStage::CatalogProjection,
    )?;
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
    inputs
        .work_budget
        .check_deadline(OperatorWorkBudgetStage::CatalogProjection)?;

    // Recompute every projected subroot hash and prove read-back equality
    // against the re-normalized tables; bind the index records exactly when
    // the conversion produced more than one table.
    let mut catalog_hashes = Vec::with_capacity(planned.len());
    for table in &planned {
        inputs
            .work_budget
            .check_deadline(OperatorWorkBudgetStage::CatalogProjection)?;
        let actual_hash = logical_catalog_hash_guarded(&table.subroot, inputs.work_budget)
            .with_context(|| format!("verify catalog hash {}", table.subroot.display()))?;
        assert_planned_read_back(table, inputs.work_budget)?;
        ensure!(
            table.canonical_path.is_file(),
            "completed conversion is missing canonical artifact {}",
            table.canonical_path.display()
        );
        catalog_hashes.push(actual_hash);
        inputs
            .work_budget
            .check_deadline(OperatorWorkBudgetStage::CatalogProjection)?;
    }
    let (actual_rows, actual_row_groups) = planned.iter().try_fold(
        (0_u64, 0_u64),
        |(rows, row_groups), table| -> Result<(u64, u64)> {
            inputs
                .work_budget
                .check_deadline(OperatorWorkBudgetStage::CatalogProjection)?;
            let metadata =
                actual_nt_market_data_metadata_guarded(&table.subroot, inputs.work_budget)?;
            inputs
                .work_budget
                .check_deadline(OperatorWorkBudgetStage::CatalogProjection)?;
            Ok((
                rows.checked_add(metadata.rows)
                    .context("completed actual projected row total overflow")?,
                row_groups
                    .checked_add(metadata.row_groups)
                    .context("completed actual projected row-group total overflow")?,
            ))
        },
    )?;
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
    inputs
        .work_budget
        .check_deadline(OperatorWorkBudgetStage::CatalogProjection)?;
    let index_records = validate_conversion_tables_index(inputs.output_dir, &conversion_manifest)?;
    inputs
        .work_budget
        .check_deadline(OperatorWorkBudgetStage::CatalogProjection)?;
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
    assert_tables_overlap_window(&local_manifest, &planned, inputs.work_budget)?;
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

    let submitted_identity = submitted_run_identity_for_spec(spec)?;
    let catalog_run_view_authority = load_catalog_run_view_authority_guarded(
        spec,
        &local_manifest,
        inputs.output_dir,
        inputs.work_budget,
    )?;

    let nt_run = run_nt_backtest_node_guarded(
        &local_manifest,
        &submitted_identity,
        &catalog_run_view_authority,
        inputs.work_budget,
    )?;
    inputs
        .work_budget
        .check_deadline(OperatorWorkBudgetStage::Backtest)?;
    let nt_result = nt_run.result;
    let config_override_report = nt_run.config_override_report;
    let run_guard_report = nt_run.run_guard_report;
    let window_start = window_bound_nanos("start_time", local_manifest.start_time)?;
    let window_end = window_bound_nanos("end_time", local_manifest.end_time)?;
    let mut expected = 0usize;
    for table in &planned {
        inputs
            .work_budget
            .check_deadline(OperatorWorkBudgetStage::Backtest)?;
        let table_count = table
            .table
            .windowed_count(window_start, window_end, inputs.work_budget)
            .context("compute expected engine iterations for projected table")?;
        expected = expected
            .checked_add(table_count)
            .context("aggregate expected engine iteration count overflow")?;
        inputs
            .work_budget
            .check_deadline(OperatorWorkBudgetStage::Backtest)?;
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
    let contract =
        verify_completed_result_contract(&inputs.contract_path, &contract, inputs.work_budget)?;
    inputs
        .work_budget
        .check_deadline(OperatorWorkBudgetStage::Finalize)?;

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
        .collect::<Vec<_>>();
    let batch_summary = OperatorRunSummary::multi(&tables, &conversion_manifest.catalog_hash)?;
    ensure!(
        batch_summary == sealed_summary,
        "completed multi-table summary changed after sealed verification"
    );

    Ok(MultiTableRunArtifacts {
        verified_sha256: inputs.verified_sha256,
        accepted_source_proof: inputs.accepted_proof,
        proof_path: inputs.proof_path,
        contract_path: inputs.contract_path,
        run_manifest_path: inputs.run_manifest_path,
        conversion_manifest_path: inputs.conversion_manifest_path,
        conversion_checkpoint_path: inputs.conversion_checkpoint_path,
        catalog_metadata_path: inputs.catalog_metadata_path,
        catalog_run_view_authority_path: inputs.output_dir.join(CATALOG_RUN_VIEW_AUTHORITY_FILE),
        conversion_tables_path,
        tables,
        conversion_checkpoint,
        conversion_manifest,
        conversion_catalog_metadata,
        conversion_checkpoint_hash: inputs.conversion_checkpoint_hash,
        conversion_manifest_hash: inputs.conversion_manifest_hash,
        nt_result,
        contract,
        batch_summary,
    })
}

#[cfg(test)]
mod tests {
    use std::{
        io::{Cursor, Write},
        sync::{
            Arc, Mutex,
            atomic::{AtomicUsize, Ordering},
        },
        time::Duration,
    };

    use crate::hashing::sha256_hex;

    use flate2::{Compression, write::GzEncoder};

    use super::*;
    use crate::backfill_execution_plan::BackfillExecutionWorkBudget;
    use crate::canonical_trades::{
        CsvTimestampUnit, FUNDING_RATES_TRANSFORM_IDENTITY, FUNDING_RATES_TRANSFORM_VERSION,
        REGISTERED_SOURCE_ADAPTERS, RawPayloadConfig, RawPayloadContainer,
    };
    use crate::conversion_boundary::{
        CATALOG_METADATA_FILE, CONVERSION_CHECKPOINT_FILE, CONVERSION_MANIFEST_FILE,
        CatalogConsumption, ConversionCatalogMetadata, ConversionCheckpoint, ConversionManifest,
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

    #[test]
    fn transient_catalog_root_lease_finish_retains_its_unique_root() {
        let output = tempfile::tempdir().expect("output dir");
        let lease = TransientCatalogRootLease::acquire(output.path())
            .expect("acquire transient catalog root");
        let catalog_root = lease.catalog_root.clone();
        fs::write(catalog_root.join("owned.parquet"), b"owned").expect("write owned catalog file");

        lease
            .finish_retained(&OperatorWorkBudgetGuard::unbounded())
            .expect("finish retained transient catalog root");

        assert_eq!(
            fs::read(catalog_root.join("owned.parquet")).expect("retained owned catalog file"),
            b"owned"
        );
        assert!(!output.path().join(CATALOG_DIR).exists());
    }

    #[test]
    fn stable_transient_catalog_root_lease_reopens_existing_root_without_cleanup() {
        let output = tempfile::tempdir().expect("output dir");
        let first = TransientCatalogRootLease::acquire_stable(output.path())
            .expect("acquire stable catalog root");
        let catalog_root = first.catalog_root.clone();
        fs::write(catalog_root.join("candidate.parquet"), b"candidate")
            .expect("write deterministic candidate");
        first
            .finish_retained(&OperatorWorkBudgetGuard::unbounded())
            .expect("finish retained stable root");

        let retry = TransientCatalogRootLease::acquire_stable(output.path())
            .expect("retry reopens stable catalog root");
        assert_eq!(retry.catalog_root, catalog_root);
        assert_eq!(
            fs::read(catalog_root.join("candidate.parquet")).expect("candidate remains"),
            b"candidate"
        );
        retry
            .finish_retained(&OperatorWorkBudgetGuard::unbounded())
            .expect("finish retained retry root");
    }

    #[cfg(unix)]
    #[test]
    fn stable_transient_catalog_root_rejects_permissive_existing_mode() {
        let output = tempfile::tempdir().expect("output dir");
        let catalog_root = output.path().join(CATALOG_DIR);
        fs::create_dir(&catalog_root).expect("preexisting catalog root");
        fs::set_permissions(&catalog_root, fs::Permissions::from_mode(0o755))
            .expect("make existing root permissive");

        let error = TransientCatalogRootLease::acquire_stable(output.path())
            .expect_err("permissive existing root must fail closed");

        assert!(error.to_string().contains("private"), "{error:#}");
        assert_eq!(
            fs::metadata(&catalog_root)
                .expect("stat preserved root")
                .permissions()
                .mode()
                & UNIX_PERMISSION_MASK,
            0o755,
            "retry validation must not chmod a foreign root"
        );
    }

    #[cfg(unix)]
    #[test]
    fn transient_catalog_root_lease_rejects_mode_change_after_acquisition() {
        let output = tempfile::tempdir().expect("output dir");
        let lease = TransientCatalogRootLease::acquire_stable(output.path())
            .expect("acquire stable catalog root");
        let catalog_root = lease.catalog_root.clone();
        fs::set_permissions(&catalog_root, fs::Permissions::from_mode(0o755))
            .expect("make leased root permissive");

        let error = lease
            .finish_retained(&OperatorWorkBudgetGuard::unbounded())
            .expect_err("lease must recheck exact private mode");

        assert!(
            error.to_string().contains("private mode changed"),
            "{error:#}"
        );
        assert_eq!(
            fs::metadata(&catalog_root)
                .expect("stat retained changed root")
                .permissions()
                .mode()
                & UNIX_PERMISSION_MASK,
            0o755,
            "failed revalidation must not mutate or delete the changed root"
        );
    }

    #[test]
    fn abandoned_unique_catalog_root_never_blocks_a_retry() {
        let output = tempfile::tempdir().expect("output dir");
        let abandoned = TransientCatalogRootLease::acquire(output.path())
            .expect("acquire first transient catalog root");
        let abandoned_root = abandoned.catalog_root.clone();
        fs::write(abandoned_root.join("partial.parquet"), b"partial")
            .expect("write abandoned partial catalog");
        std::mem::forget(abandoned);

        let retry = TransientCatalogRootLease::acquire(output.path())
            .expect("retry acquires a different transient catalog root");
        let retry_root = retry.catalog_root.clone();

        assert_ne!(retry_root, abandoned_root);
        assert!(abandoned_root.join("partial.parquet").exists());
        assert!(!output.path().join(CATALOG_DIR).exists());
        retry
            .finish_retained(&OperatorWorkBudgetGuard::unbounded())
            .expect("finish retained retry root");
        assert!(retry_root.is_dir(), "finished retry root is retained");
    }

    #[cfg(unix)]
    #[test]
    fn transient_catalog_root_lease_rejects_replacement_without_deleting_it() {
        let output = tempfile::tempdir().expect("output dir");
        let lease = TransientCatalogRootLease::acquire(output.path())
            .expect("acquire transient catalog root");
        let catalog_root = lease.catalog_root.clone();
        let displaced_root = output.path().join("displaced-catalog");
        fs::rename(&catalog_root, &displaced_root).expect("displace owned root");
        fs::create_dir(&catalog_root).expect("replacement catalog root");
        fs::write(catalog_root.join("foreign.parquet"), b"foreign")
            .expect("write replacement content");

        let error = lease
            .finish_retained(&OperatorWorkBudgetGuard::unbounded())
            .expect_err("identity replacement must fail closed before finish");

        assert!(error.to_string().contains("identity changed"), "{error:#}");
        assert_eq!(
            fs::read(catalog_root.join("foreign.parquet")).expect("replacement remains"),
            b"foreign"
        );
    }

    #[test]
    fn transient_catalog_root_finish_deadline_retains_owned_residue() {
        let output = tempfile::tempdir().expect("output dir");
        let lease = TransientCatalogRootLease::acquire(output.path())
            .expect("acquire transient catalog root");
        let catalog_root = lease.catalog_root.clone();
        fs::write(catalog_root.join("partial.parquet"), b"partial").expect("write partial catalog");

        let error = lease
            .finish_retained(&expiring_test_work_budget(0))
            .expect_err("expired finish must fail closed");

        assert!(error.to_string().contains("max_wall_seconds"), "{error:#}");
        assert_eq!(
            fs::read(catalog_root.join("partial.parquet")).expect("retained partial catalog"),
            b"partial"
        );
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

    fn test_work_budget(
        max_source_rows: u64,
        max_projected_row_groups: u64,
    ) -> OperatorWorkBudgetGuard {
        OperatorWorkBudgetGuard::new(crate::operator_work_budget::OperatorWorkBudget::Backfill(
            crate::backfill_execution_plan::BackfillExecutionWorkBudget {
                max_decoded_bytes: u64::MAX,
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

    struct ExpiringReadClock {
        observations: AtomicUsize,
        expires_after_observation: usize,
    }

    impl crate::operator_work_budget::OperatorWorkBudgetClock for ExpiringReadClock {
        fn now(&self) -> Duration {
            if self.observations.fetch_add(1, Ordering::SeqCst) >= self.expires_after_observation {
                Duration::from_secs(1)
            } else {
                Duration::ZERO
            }
        }
    }

    fn expiring_test_work_budget(expires_after_observation: usize) -> OperatorWorkBudgetGuard {
        OperatorWorkBudgetGuard::with_clock(
            crate::operator_work_budget::OperatorWorkBudget::Backfill(
                crate::backfill_execution_plan::BackfillExecutionWorkBudget {
                    max_decoded_bytes: u64::MAX,
                    max_source_rows: 1,
                    max_projected_row_groups: 1,
                    max_wall_seconds: 1,
                    require_object_selection_metadata: false,
                },
            ),
            Arc::new(ExpiringReadClock {
                observations: AtomicUsize::new(0),
                expires_after_observation,
            }),
        )
        .expect("expiring guard")
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
        spec.artifact_store = None;
        spec
    }

    fn durable_run_spec_rejected_by_local_entries(gz_bytes: &[u8]) -> RunSpec {
        let durable: RunSpec =
            toml::from_str(COMMITTED_RUN_SPEC).expect("committed durable run-spec parses");
        let mut spec = run_spec_for(gz_bytes);
        spec.artifact_store = durable.artifact_store;
        spec.source_bindings_path = PathBuf::from("must-not-read/source-bindings.toml");
        spec
    }

    fn assert_local_entry_rejected_before_output<T>(
        result: Result<T>,
        output_dir: &Path,
        entry: &str,
    ) {
        let error = match result {
            Ok(_) => panic!("{entry} must reject a durable RunSpec"),
            Err(error) => error,
        };
        assert!(
            error
                .to_string()
                .contains("must use source_universe_batch_execution"),
            "{entry}: {error:#}"
        );
        assert!(
            !error.to_string().contains("must-not-read"),
            "{entry} must reject before reading the source-binding registry: {error:#}"
        );
        assert!(
            !output_dir.exists(),
            "{entry} must reject before creating operator output"
        );
    }

    #[test]
    fn public_trade_entry_rejects_durable_run_spec_before_registry_or_output() {
        let object_bytes = gzip(SAMPLE_CSV);
        let spec = durable_run_spec_rejected_by_local_entries(&object_bytes);
        let temp = tempfile::tempdir().expect("temp dir");
        let output_dir = temp.path().join("output");

        assert_local_entry_rejected_before_output(
            run_from_run_spec(&spec, &object_bytes, &output_dir),
            &output_dir,
            "run_from_run_spec",
        );
    }

    #[test]
    fn public_operator_entry_rejects_durable_run_spec_before_registry_or_output() {
        let object_bytes = gzip(SAMPLE_CSV);
        let spec = durable_run_spec_rejected_by_local_entries(&object_bytes);
        let temp = tempfile::tempdir().expect("temp dir");
        let output_dir = temp.path().join("output");

        assert_local_entry_rejected_before_output(
            run_operator_from_run_spec(&spec, &object_bytes, &output_dir),
            &output_dir,
            "run_operator_from_run_spec",
        );
    }

    #[test]
    fn public_multi_table_entry_rejects_durable_run_spec_before_registry_or_output() {
        let object_bytes = gzip(SAMPLE_CSV);
        let spec = durable_run_spec_rejected_by_local_entries(&object_bytes);
        let temp = tempfile::tempdir().expect("temp dir");
        let output_dir = temp.path().join("output");

        assert_local_entry_rejected_before_output(
            run_multi_table_from_run_spec(&spec, &object_bytes, &output_dir),
            &output_dir,
            "run_multi_table_from_run_spec",
        );
    }

    #[test]
    fn public_guarded_entries_reject_durable_run_spec_before_using_registry_or_output() {
        let object_bytes = gzip(SAMPLE_CSV);
        let local_spec = run_spec_for(&object_bytes);
        let registry = VerifiedSourceBindingRegistry::from_run_spec(&local_spec)
            .expect("freeze local source-binding registry");
        let spec = durable_run_spec_rejected_by_local_entries(&object_bytes);
        let temp = tempfile::tempdir().expect("temp dir");
        let trade_output = temp.path().join("trade-output");
        let operator_output = temp.path().join("operator-output");
        let work_budget = OperatorWorkBudgetGuard::unbounded();

        assert_local_entry_rejected_before_output(
            run_from_run_spec_guarded(&spec, &object_bytes, &trade_output, &work_budget),
            &trade_output,
            "run_from_run_spec_guarded",
        );
        assert_local_entry_rejected_before_output(
            run_operator_from_run_spec_guarded(
                &spec,
                &object_bytes,
                &operator_output,
                &registry,
                &work_budget,
            ),
            &operator_output,
            "run_operator_from_run_spec_guarded",
        );
    }

    #[test]
    fn verified_registry_handle_cannot_authorize_a_different_run_spec_path() {
        let object_bytes = gzip(SAMPLE_CSV);
        let original = run_spec_for(&object_bytes);
        let verified = VerifiedSourceBindingRegistry::from_run_spec(&original)
            .expect("verify original source bindings");
        let mut changed = original;
        changed.source_bindings_path = PathBuf::from("different-source-bindings.toml");
        let output = tempfile::tempdir().expect("output dir");

        let error = match run_from_run_spec_with_verified_registry(
            &changed,
            &object_bytes,
            output.path(),
            &verified,
            &OperatorWorkBudgetGuard::unbounded(),
        ) {
            Ok(_) => panic!("verified registry must not authorize changed control identity"),
            Err(error) => error,
        };

        assert!(
            error
                .to_string()
                .contains("verified source-bindings path mismatch"),
            "{error:#}"
        );
        assert!(
            !output.path().join(CONVERSION_CHECKPOINT_FILE).exists(),
            "registry identity rejection must precede artifact writes"
        );
    }

    #[test]
    fn verified_registry_guard_rejects_snapshot_above_work_budget_before_reading() {
        let object_bytes = gzip(SAMPLE_CSV);
        let spec = run_spec_for(&object_bytes);
        let work_budget = OperatorWorkBudgetGuard::new(
            crate::operator_work_budget::OperatorWorkBudget::Backfill(
                BackfillExecutionWorkBudget {
                    max_decoded_bytes: 1,
                    max_source_rows: 1,
                    max_projected_row_groups: 1,
                    max_wall_seconds: 1,
                    require_object_selection_metadata: false,
                },
            ),
        )
        .expect("bounded source-binding registry budget");

        let error = VerifiedSourceBindingRegistry::from_run_spec_guarded(&spec, &work_budget)
            .expect_err("source-binding snapshot must obey the execution-plan byte ceiling");

        assert!(error.to_string().contains("max_decoded_bytes"), "{error:#}");
    }

    #[test]
    fn verified_registry_rejects_retired_path_before_filesystem_access() {
        let object_bytes = gzip(SAMPLE_CSV);
        let mut spec = run_spec_for(&object_bytes);
        spec.source_bindings_path = PathBuf::from(
            "specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-03-02/materialized-run-spec/backfill-run-spec.toml",
        );

        let error = VerifiedSourceBindingRegistry::from_run_spec(&spec)
            .expect_err("retired source-bindings path must reject before absence");

        assert!(error.to_string().contains("retired backfill"), "{error:#}");
        assert!(
            !error.to_string().contains("No such file"),
            "retirement policy must reject before absence happens to reject: {error:#}"
        );
    }

    #[test]
    fn verified_registry_executes_from_frozen_bytes_without_reopening_path() {
        let object_bytes = gzip(SAMPLE_CSV);
        let mut spec = run_spec_for(&object_bytes);
        let fixture = tempfile::tempdir().expect("fixture dir");
        let registry_bytes = fs::read(crate::source_proof::resolve_source_bindings_path(
            &spec.source_bindings_path,
        ))
        .expect("read committed source bindings");
        let registry_path = fixture.path().join("source-bindings.toml");
        fs::write(&registry_path, &registry_bytes).expect("write registry snapshot source");
        spec.source_bindings_path = registry_path.clone();
        let verified = VerifiedSourceBindingRegistry::from_run_spec(&spec)
            .expect("freeze source-binding registry");
        let expected_sha256 = verified.sha256().to_string();
        fs::write(&registry_path, b"not = [valid toml")
            .expect("replace registry path after snapshot");
        let output_dir = fixture.path().join("output");

        let artifacts = run_from_run_spec_with_verified_registry(
            &spec,
            &object_bytes,
            &output_dir,
            &verified,
            &OperatorWorkBudgetGuard::unbounded(),
        )
        .expect("frozen registry remains sole execution input");

        assert_eq!(
            artifacts
                .output
                .conversion_manifest
                .fingerprint
                .control_artifact_sha256,
            expected_sha256
        );
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

    fn decode_test_payload(
        config: &RawPayloadConfig,
        object_bytes: &[u8],
    ) -> Result<DecodedPayload> {
        decode_object_payload(config, object_bytes, &OperatorWorkBudgetGuard::unbounded())
    }

    #[test]
    fn tiny_decode_does_not_allocate_from_a_huge_declared_ceiling() {
        let payload = b"tiny";
        let bytes = read_limited_bytes(
            Cursor::new(payload),
            5_u64 * 1_024 * 1_024 * 1_024,
            "tiny payload with five-GiB ceiling",
            &OperatorWorkBudgetGuard::unbounded(),
        )
        .expect("the ceiling is a rejection bound, not an allocation request");

        assert_eq!(bytes, payload);
        assert!(bytes.capacity() < 1_024 * 1_024);
    }

    #[test]
    fn decode_jsonl_text_payload_decodes_within_bound() {
        let config = payload_config(RawPayloadContainer::JsonlText);
        let payload =
            decode_test_payload(&config, b"{\"a\":1}\n{\"a\":2}\n").expect("jsonl text decodes");
        match payload {
            DecodedPayload::Text(text) => assert_eq!(text, "{\"a\":1}\n{\"a\":2}\n"),
            DecodedPayload::TarMembers(_) | DecodedPayload::ParquetBytes(_) => {
                panic!("jsonl text container must decode to a text payload")
            }
        }
    }

    #[test]
    fn decode_stops_when_deadline_expires_between_guarded_reads() {
        let mut config = payload_config(RawPayloadContainer::JsonlText);
        config.max_object_bytes = 262_144;
        config.max_decoded_bytes = 262_144;
        let clock = Arc::new(ExpiringReadClock {
            observations: AtomicUsize::new(0),
            expires_after_observation: 3,
        });
        let guard = OperatorWorkBudgetGuard::with_clock(
            crate::operator_work_budget::OperatorWorkBudget::Backfill(
                crate::backfill_execution_plan::BackfillExecutionWorkBudget {
                    max_decoded_bytes: u64::MAX,
                    max_source_rows: 1,
                    max_projected_row_groups: 1,
                    max_wall_seconds: 1,
                    require_object_selection_metadata: false,
                },
            ),
            clock,
        )
        .expect("guard");
        let bytes = vec![b'x'; 131_072];

        let error = decode_object_payload(&config, &bytes, &guard)
            .err()
            .expect("decode must stop at the cooperative deadline");

        assert!(error.to_string().contains("max_wall_seconds"), "{error:#}");
        assert!(error.to_string().contains("decode"), "{error:#}");
    }

    #[test]
    fn gzip_decode_stops_when_deadline_expires_between_decompressed_reads() {
        let mut config = payload_config(RawPayloadContainer::JsonlGzip);
        config.max_object_bytes = 262_144;
        config.max_decoded_bytes = 262_144;
        let compressed = gzip(&"x".repeat(131_072));
        let guard = expiring_test_work_budget(5);

        let error = decode_object_payload(&config, &compressed, &guard)
            .err()
            .expect("gzip decode must stop at the cooperative deadline");

        assert!(error.to_string().contains("max_wall_seconds"), "{error:#}");
        assert!(error.to_string().contains("decode"), "{error:#}");
    }

    #[test]
    fn zip_decode_stops_when_deadline_expires_between_member_reads() {
        let mut config = payload_config(RawPayloadContainer::SingleJsonlZip);
        config.max_object_bytes = 262_144;
        config.max_decoded_bytes = 262_144;
        let compressed = zip_single_csv("book.data", &"x".repeat(131_072));
        let guard = expiring_test_work_budget(5);

        let error = decode_object_payload(&config, &compressed, &guard)
            .err()
            .expect("ZIP decode must stop at the cooperative deadline");

        assert!(error.to_string().contains("max_wall_seconds"), "{error:#}");
        assert!(error.to_string().contains("decode"), "{error:#}");
    }

    #[test]
    fn tar_decode_stops_when_deadline_expires_mid_matching_member() {
        let mut config = payload_config(RawPayloadContainer::TarGzipJsonl);
        config.max_object_bytes = 262_144;
        config.member_suffix = Some(".jsonl".to_string());
        config.max_member_bytes = Some(2_048);
        let member = vec![b'x'; TEST_TAR_BLOCK * 3];
        let archive = gzip_tar(&[("large.jsonl", member.as_slice())]);
        // Guard construction + decode entry + one header block + the first
        // member block remain in budget; the next 512-byte member read expires.
        let guard = expiring_test_work_budget(7);

        let error = decode_object_payload(&config, &archive, &guard)
            .err()
            .expect("tar decode must stop inside a multi-block member");

        assert!(error.to_string().contains("max_wall_seconds"), "{error:#}");
        assert!(error.to_string().contains("decode"), "{error:#}");
    }

    #[test]
    fn nonterminal_stage_checks_deadline_after_its_operation() {
        let clock = Arc::new(TestWorkBudgetClock::default());
        let guard = OperatorWorkBudgetGuard::with_clock(
            crate::operator_work_budget::OperatorWorkBudget::Backfill(
                crate::backfill_execution_plan::BackfillExecutionWorkBudget {
                    max_decoded_bytes: u64::MAX,
                    max_source_rows: 1,
                    max_projected_row_groups: 1,
                    max_wall_seconds: 1,
                    require_object_selection_metadata: false,
                },
            ),
            clock.clone(),
        )
        .expect("guard");
        let output = tempfile::NamedTempFile::new().expect("output file");

        let error = run_budgeted_stage(&guard, OperatorWorkBudgetStage::Finalize, || {
            fs::write(output.path(), b"nonterminal finalization bytes")?;
            clock.set(Duration::from_secs(1));
            Ok(())
        })
        .expect_err("expiry after a nonterminal operation must be observed");

        assert!(error.to_string().contains("max_wall_seconds"), "{error:#}");
        assert_eq!(
            fs::read(output.path()).expect("written nonterminal bytes"),
            b"nonterminal finalization bytes"
        );
    }

    #[test]
    fn decode_jsonl_text_payload_rejects_decoded_bytes_over_bound() {
        let config = payload_config(RawPayloadContainer::JsonlText);
        let oversize = vec![b'x'; 65];
        let err = decode_test_payload(&config, &oversize)
            .err()
            .expect("over-bound jsonl text must be rejected");
        assert!(err.to_string().contains("max_decoded_bytes"), "{err}");
    }

    #[test]
    fn decode_jsonl_gzip_payload_decodes_and_bounds_decoded_bytes() {
        let config = payload_config(RawPayloadContainer::JsonlGzip);
        let payload =
            decode_test_payload(&config, &gzip("{\"a\":1}\n")).expect("jsonl gzip decodes");
        match payload {
            DecodedPayload::Text(text) => assert_eq!(text, "{\"a\":1}\n"),
            DecodedPayload::TarMembers(_) | DecodedPayload::ParquetBytes(_) => {
                panic!("jsonl gzip container must decode to a text payload")
            }
        }

        let oversize_text = "y".repeat(65);
        let err = decode_test_payload(&config, &gzip(&oversize_text))
            .err()
            .expect("over-bound decoded jsonl gzip must be rejected");
        assert!(err.to_string().contains("max_decoded_bytes"), "{err}");
    }

    #[test]
    fn decode_single_jsonl_zip_payload_decodes_with_crc_verification() {
        let mut config = payload_config(RawPayloadContainer::SingleJsonlZip);
        config.max_decoded_bytes = 128;
        let payload = decode_test_payload(&config, &zip_single_csv("book.data", "{\"a\":1}\n"))
            .expect("jsonl zip decodes");
        match payload {
            DecodedPayload::Text(text) => assert_eq!(text, "{\"a\":1}\n"),
            DecodedPayload::TarMembers(_) | DecodedPayload::ParquetBytes(_) => {
                panic!("jsonl zip container must decode to a text payload")
            }
        }

        config.max_decoded_bytes = 4;
        let err = decode_test_payload(&config, &zip_single_csv("book.data", "{\"a\":1}\n"))
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
        let payload = decode_test_payload(&config, &archive).expect("tar gzip decodes");
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
        let err = decode_test_payload(&config, &archive)
            .err()
            .expect("over-bound tar member must be rejected");
        assert!(
            err.to_string().contains("big.jsonl") || err.to_string().contains("member"),
            "{err}"
        );
    }

    #[test]
    fn decode_tar_gzip_jsonl_rejects_cumulative_members_over_decoded_bound() {
        let mut config = payload_config(RawPayloadContainer::TarGzipJsonl);
        config.member_suffix = Some(".jsonl".to_string());
        config.max_member_bytes = Some(8);
        config.max_decoded_bytes = 10;
        let archive = gzip_tar(&[
            ("a.jsonl", b"123456".as_slice()),
            ("b.jsonl", b"abcdef".as_slice()),
        ]);
        let error = decode_test_payload(&config, &archive)
            .err()
            .expect("aggregate decoded bytes over the configured bound must be rejected");
        assert!(error.to_string().contains("max_decoded_bytes"), "{error:#}");
    }

    #[test]
    fn decode_parquet_file_passes_object_bytes_through() {
        let config = payload_config(RawPayloadContainer::ParquetFile);
        let bytes = b"PAR1synthetic-not-read-here".to_vec();
        let payload = decode_test_payload(&config, &bytes).expect("parquet passthrough");
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
        assert_eq!(
            artifacts.output.projection.catalog_root, artifacts.catalog_root,
            "fresh runtime manifest must bind the deterministic catalog root"
        );
        assert_eq!(
            artifacts.catalog_root,
            dir.path().join(CATALOG_DIR),
            "local retries must project through one stable catalog pathname"
        );
        let mut runtime_manifest = spec.manifest.clone();
        bind_runtime_manifest_to_local_catalog_root(&mut runtime_manifest, &artifacts.catalog_root)
            .expect("bind transient runtime location");
        let submitted_identity = submitted_run_identity_for_spec(&spec)
            .expect("derive identity from immutable submitted manifest");
        artifacts
            .output
            .catalog_run_view_authority
            .validate_for_runtime_manifest(
                &runtime_manifest,
                &submitted_identity,
                &OperatorWorkBudgetGuard::unbounded(),
                OperatorWorkBudgetStage::Backtest,
            )
            .expect("catalog location-only rewrite remains authorized");
        runtime_manifest.nt_streaming_chunk_size += 1;
        let semantic_error = artifacts
            .output
            .catalog_run_view_authority
            .validate_for_runtime_manifest(
                &runtime_manifest,
                &submitted_identity,
                &OperatorWorkBudgetGuard::unbounded(),
                OperatorWorkBudgetStage::Backtest,
            )
            .expect_err("runtime semantic rewrite must be rejected");
        assert!(
            semantic_error
                .to_string()
                .contains("outside the allowed catalog location rewrite"),
            "{semantic_error}"
        );
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
    fn completed_output_requires_an_exact_terminal_seal() {
        let gz = gzip(SAMPLE_CSV);
        let spec = run_spec_for(&gz);
        let dir = tempfile::TempDir::new().unwrap();
        let artifacts = run_from_run_spec(&spec, &gz, dir.path()).expect("operator run");
        let registry = VerifiedSourceBindingRegistry::from_run_spec(&spec)
            .expect("verify source-binding registry");
        let work_budget = OperatorWorkBudgetGuard::unbounded();
        let seal_path = dir.path().join(OPERATOR_TERMINAL_SEAL_FILE);

        assert!(seal_path.is_file(), "operator terminal seal written last");
        verify_completed_operator_output(&spec, dir.path(), &registry, &work_budget)
            .expect("intact sealed output verifies");

        for path in [
            artifacts.canonical_artifact_path,
            artifacts.contract_path,
            artifacts.proof_path,
            artifacts.run_manifest_path,
            dir.path().join(CATALOG_RUN_VIEW_AUTHORITY_FILE),
        ] {
            let original = fs::read(&path).expect("read sealed artifact");
            fs::write(&path, b"corrupt after terminal seal").expect("corrupt sealed artifact");
            let error =
                verify_completed_operator_output(&spec, dir.path(), &registry, &work_budget)
                    .expect_err("corrupt sealed artifact must reject resume");
            assert!(
                error.to_string().contains("terminal seal"),
                "corruption must fail at the exact-set seal: {error:#}"
            );
            fs::write(&path, original).expect("restore sealed artifact");
            verify_completed_operator_output(&spec, dir.path(), &registry, &work_budget)
                .expect("restored sealed output verifies");
        }

        let stray_path = dir.path().join("late-addition.parquet");
        fs::write(&stray_path, b"late addition").expect("plant stray file");
        let error = verify_completed_operator_output(&spec, dir.path(), &registry, &work_budget)
            .expect_err("stray file must reject resume");
        assert!(
            error.to_string().contains("terminal seal"),
            "stray file must fail at the exact-set seal: {error:#}"
        );
        fs::remove_file(&stray_path).expect("remove test stray");

        fs::remove_file(&seal_path).expect("remove terminal seal");
        run_from_run_spec(&spec, &gz, dir.path())
            .expect("completed checkpoint without a seal must deterministically finalize");
        assert!(
            seal_path.is_file(),
            "retry recreates the sole terminal marker"
        );
        verify_completed_operator_output(&spec, dir.path(), &registry, &work_budget)
            .expect("re-finalized output verifies");
    }

    #[test]
    fn retry_after_catalog_authority_crash_reconciles_immutable_artifacts() {
        let gz = gzip(SAMPLE_CSV);
        let spec = run_spec_for(&gz);
        let dir = tempfile::TempDir::new().unwrap();
        let artifacts = run_from_run_spec(&spec, &gz, dir.path()).expect("operator run");
        let authority_path = dir.path().join(CATALOG_RUN_VIEW_AUTHORITY_FILE);
        let authority_before = fs::read(&authority_path).expect("read authority");

        for path in [
            dir.path().join(OPERATOR_TERMINAL_SEAL_FILE),
            artifacts.conversion_checkpoint_path,
            artifacts.conversion_manifest_path,
            artifacts.catalog_metadata_path,
            artifacts.proof_path,
            artifacts.contract_path,
            artifacts.run_manifest_path,
        ] {
            fs::remove_file(path).expect("simulate crash before local completion");
        }

        run_from_run_spec(&spec, &gz, dir.path())
            .expect("retry must reconcile the existing canonical catalog and authority");
        assert_eq!(
            fs::read(&authority_path).expect("read reconciled authority"),
            authority_before,
            "retry must verify rather than replace the existing authority"
        );
        assert!(
            dir.path().join(OPERATOR_TERMINAL_SEAL_FILE).is_file(),
            "retry reaches the sole local terminal marker"
        );
    }

    #[test]
    fn retry_rejects_conflicting_deterministic_artifact_without_replacement() {
        let gz = gzip(SAMPLE_CSV);
        let spec = run_spec_for(&gz);
        let dir = tempfile::TempDir::new().unwrap();
        let artifacts = run_from_run_spec(&spec, &gz, dir.path()).expect("operator run");
        fs::remove_file(dir.path().join(OPERATOR_TERMINAL_SEAL_FILE))
            .expect("simulate missing terminal seal");
        let conflict = b"foreign result contract";
        fs::write(&artifacts.contract_path, conflict).expect("plant conflict");

        let error = run_from_run_spec(&spec, &gz, dir.path())
            .err()
            .expect("retry must reject conflicting immutable bytes");
        assert!(
            format!("{error:#}").contains("different bytes"),
            "{error:#}"
        );
        assert_eq!(
            fs::read(&artifacts.contract_path).expect("read preserved conflict"),
            conflict,
            "conflict handling must not overwrite the existing target"
        );
        assert!(!dir.path().join(OPERATOR_TERMINAL_SEAL_FILE).exists());
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
        let expected_source_bindings_sha256 = sha256_hex(
            &fs::read(crate::source_proof::resolve_source_bindings_path(
                &spec.source_bindings_path,
            ))
            .expect("read committed source bindings"),
        );
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
        assert_eq!(
            manifest.fingerprint.control_artifact_path,
            spec.source_bindings_path.to_str().unwrap()
        );
        assert_eq!(
            manifest.fingerprint.control_artifact_sha256,
            expected_source_bindings_sha256
        );
        assert_eq!(
            checkpoint.fingerprint.control_artifact_path,
            spec.source_bindings_path.to_str().unwrap()
        );
        assert_eq!(
            checkpoint.fingerprint.control_artifact_sha256,
            expected_source_bindings_sha256
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
        assert!(
            matches!(metadata.catalog_consumption, CatalogConsumption::Unproven),
            "fresh catalog metadata must not persist a transient local execution path"
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
    fn hydrated_receipt_metadata_and_contract_are_retry_stable_across_local_roots() {
        let gz = gzip(SAMPLE_CSV);
        let spec = run_spec_for(&gz);
        let dir = tempfile::TempDir::new().unwrap();
        let artifacts = run_from_run_spec(&spec, &gz, dir.path()).expect("operator run");
        let receipt = CatalogPublicationReceiptIdentity {
            catalog_root_uri: "s3://bolt-parquet/nt-catalog/v1/projection=test/".to_string(),
            receipt_uri: "s3://bolt-parquet/nt-catalog/v1/projection=test/publication-receipt.json"
                .to_string(),
            receipt_sha256: "1".repeat(64),
            receipt_version_id: "receipt-version".to_string(),
            physical_manifest_sha256: "2".repeat(64),
        };
        let bind_retry = |local_catalog_root: PathBuf| {
            artifacts
                .output
                .conversion_catalog_metadata
                .clone()
                .with_catalog_consumption_evidence(
                    CatalogConsumptionEvidence::HydratedPublication {
                        local_catalog_root,
                        receipt: receipt.clone(),
                    },
                )
                .expect("bind hydrated retry evidence")
        };
        let first_metadata = bind_retry(dir.path().join("hydration-attempt-a"));
        let second_metadata = bind_retry(dir.path().join("hydration-attempt-b"));

        assert_eq!(first_metadata, second_metadata);
        assert_eq!(
            serde_json::to_vec(&first_metadata).unwrap(),
            serde_json::to_vec(&second_metadata).unwrap(),
            "private hydration roots must not alter persisted metadata bytes"
        );

        let bind_contract = |metadata: &ConversionCatalogMetadata| {
            let mut contract = artifacts.output.contract.clone();
            contract.catalog_metadata_hash = metadata.content_hash().unwrap();
            contract.artifact_uris.nt_catalog_uri = receipt.catalog_root_uri.clone();
            contract.artifact_uris.nt_catalog_manifest_uri = Some(receipt.receipt_uri.clone());
            serde_json::to_vec(&contract).unwrap()
        };
        assert_eq!(
            bind_contract(&first_metadata),
            bind_contract(&second_metadata),
            "retry-local hydration roots must not alter result-contract bytes"
        );
    }

    #[test]
    fn published_catalog_proof_binds_receipt_result_and_run_spec() {
        let gz = gzip(SAMPLE_CSV);
        let spec = run_spec_for(&gz);
        let dir = tempfile::TempDir::new().unwrap();
        let artifacts = run_from_run_spec(&spec, &gz, dir.path()).expect("operator run");
        let receipt = CatalogPublicationReceiptIdentity {
            catalog_root_uri: "s3://bolt-parquet/nt-catalog/v1/projection=test/".to_string(),
            receipt_uri: "s3://bolt-parquet/nt-catalog/v1/projection=test/publication-receipt.json"
                .to_string(),
            receipt_sha256: "1".repeat(64),
            receipt_version_id: "receipt-version".to_string(),
            physical_manifest_sha256: "2".repeat(64),
        };
        let metadata = artifacts
            .output
            .conversion_catalog_metadata
            .clone()
            .with_catalog_consumption_evidence(CatalogConsumptionEvidence::HydratedPublication {
                local_catalog_root: dir.path().join("hydrated-catalog"),
                receipt: receipt.clone(),
            })
            .expect("bind hydrated receipt");
        let mut contract = artifacts.output.contract.clone();
        contract.catalog_metadata_hash = metadata.content_hash().unwrap();
        contract.artifact_uris.nt_catalog_uri = receipt.catalog_root_uri.clone();
        contract.artifact_uris.nt_catalog_manifest_uri = Some(receipt.receipt_uri.clone());
        let iterations = usize::try_from(contract.nt_result.iterations).unwrap();
        let proof = PublishedCatalogProof {
            proof_version: PUBLISHED_CATALOG_PROOF_VERSION.to_string(),
            catalog_uri: receipt.catalog_root_uri.clone(),
            catalog_fs_protocol: "s3".to_string(),
            publication_receipt_uri: receipt.receipt_uri.clone(),
            publication_receipt_sha256: receipt.receipt_sha256.clone(),
            publication_receipt_version_id: receipt.receipt_version_id.clone(),
            publication_physical_manifest_sha256: receipt.physical_manifest_sha256.clone(),
            expected_iterations: iterations,
            nt_iterations: iterations,
            run_config_id: contract.nt_result.run_config_id.clone(),
            nt_version: contract.nt_version.clone(),
            created_at: contract.created_at.clone(),
        };

        proof
            .validate_against(&metadata, &contract, &spec)
            .expect("fully bound proof");

        let mut wrong_scheme = proof.clone();
        wrong_scheme.catalog_fs_protocol = "file".to_string();
        assert!(
            wrong_scheme
                .validate_against(&metadata, &contract, &spec)
                .is_err()
        );
        let mut wrong_result = proof.clone();
        wrong_result.expected_iterations += 1;
        assert!(
            wrong_result
                .validate_against(&metadata, &contract, &spec)
                .is_err()
        );
        let mut wrong_run_config = proof;
        wrong_run_config.run_config_id = Some("other-run-config".to_string());
        assert!(
            wrong_run_config
                .validate_against(&metadata, &contract, &spec)
                .is_err()
        );
    }

    #[test]
    fn run_from_run_spec_rejects_tampered_object() {
        // The committed run-spec pins the real (uncommitted) object hash; feeding
        // it the synthetic bytes must trip the SHA-256 re-verification.
        let gz = gzip(SAMPLE_CSV);
        let mut spec: RunSpec = toml::from_str(COMMITTED_RUN_SPEC).expect("parse");
        spec.artifact_store = None;
        spec.accepted_object.bytes = gz.len() as u64;
        let dir = tempfile::TempDir::new().unwrap();
        let err = run_from_run_spec(&spec, &gz, dir.path())
            .err()
            .expect("tampered object must be rejected");
        assert!(err.to_string().contains("SHA-256"), "{err}");
    }

    #[test]
    fn run_from_run_spec_rejects_unexpected_preterminal_residue_at_seal() {
        let gz = gzip(SAMPLE_CSV);
        let spec = run_spec_for(&gz);
        let dir = tempfile::TempDir::new().unwrap();
        fs::write(dir.path().join("stale.parquet"), b"stale").unwrap();

        let err = run_from_run_spec(&spec, &gz, dir.path())
            .err()
            .expect("dirty output must be rejected");

        assert!(format!("{err:#}").contains("unexpected"), "{err:#}");
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
            .err()
            .expect("third source record must exceed the two-row budget");

        assert!(error.to_string().contains("max_source_rows"), "{error:#}");
        assert!(
            !dir.path().join(CONVERSION_CHECKPOINT_FILE).exists(),
            "preterminal failure must not persist a mutable started checkpoint"
        );
    }

    #[test]
    fn fresh_row_group_budget_failure_precedes_canonical_and_catalog_writes() {
        let gz = gzip(SAMPLE_CSV);
        let spec = run_spec_for(&gz);
        let dir = tempfile::TempDir::new().unwrap();
        let guard = test_work_budget(100, 0);

        let error = run_from_run_spec_guarded(&spec, &gz, dir.path(), &guard)
            .err()
            .expect("one projected row group must exceed a zero-row-group budget");

        assert!(
            error.to_string().contains("max_projected_row_groups"),
            "{error:#}"
        );
        assert!(
            !dir.path().join(CANONICAL_ARTIFACT_FILE).exists(),
            "canonical bytes must not exist after projection-budget rejection"
        );
        assert!(
            !dir.path().join(CATALOG_DIR).exists(),
            "catalog bytes must not exist after projection-budget rejection"
        );
        assert!(
            !dir.path().join(CONVERSION_CHECKPOINT_FILE).exists(),
            "preterminal rejection must not persist a mutable started checkpoint"
        );
    }

    #[test]
    fn completed_output_is_revalidated_against_a_stricter_source_budget() {
        let gz = gzip(SAMPLE_CSV);
        let spec = run_spec_for(&gz);
        let dir = tempfile::TempDir::new().unwrap();
        run_from_run_spec(&spec, &gz, dir.path()).expect("first run completes");
        let guard = test_work_budget(2, 1);

        let error = run_from_run_spec_guarded(&spec, &gz, dir.path(), &guard)
            .err()
            .expect("completed output must not carry across a stricter source budget");

        assert!(error.to_string().contains("max_source_rows"), "{error:#}");
        assert_eq!(guard.source_rows_consumed(), 3);
    }

    #[test]
    fn completed_output_row_group_rejection_preserves_existing_bytes() {
        let gz = gzip(SAMPLE_CSV);
        let spec = run_spec_for(&gz);
        let dir = tempfile::TempDir::new().unwrap();
        let first = run_from_run_spec(&spec, &gz, dir.path()).expect("first run completes");
        let canonical_before =
            fs::read(&first.canonical_artifact_path).expect("read canonical bytes");
        let checkpoint_before =
            fs::read(&first.conversion_checkpoint_path).expect("read checkpoint bytes");
        let manifest_before =
            fs::read(&first.conversion_manifest_path).expect("read manifest bytes");
        let metadata_before = fs::read(&first.catalog_metadata_path).expect("read metadata bytes");
        let catalog_hash_before =
            crate::catalog_projection::logical_catalog_hash(&first.catalog_root)
                .expect("hash completed catalog");
        let guard = test_work_budget(100, 0);

        let error = run_from_run_spec_guarded(&spec, &gz, dir.path(), &guard)
            .err()
            .expect("completed output must be rejected by a zero-row-group budget");

        assert!(
            error.to_string().contains("max_projected_row_groups"),
            "{error:#}"
        );
        assert_eq!(
            fs::read(&first.canonical_artifact_path).expect("canonical remains"),
            canonical_before
        );
        assert_eq!(
            fs::read(&first.conversion_checkpoint_path).expect("checkpoint remains"),
            checkpoint_before
        );
        assert_eq!(
            fs::read(&first.conversion_manifest_path).expect("manifest remains"),
            manifest_before
        );
        assert_eq!(
            fs::read(&first.catalog_metadata_path).expect("metadata remains"),
            metadata_before
        );
        assert_eq!(
            crate::catalog_projection::logical_catalog_hash(&first.catalog_root)
                .expect("catalog remains"),
            catalog_hash_before
        );
    }

    #[test]
    fn run_from_run_spec_rejects_a_stray_file_in_a_completed_catalog() {
        let gz = gzip(SAMPLE_CSV);
        let spec = run_spec_for(&gz);
        let dir = tempfile::TempDir::new().unwrap();
        let first = run_from_run_spec(&spec, &gz, dir.path()).expect("first run");
        let stray = first.catalog_root.join("stray.parquet");
        fs::write(&stray, b"not part of the committed exact set").expect("plant stray file");

        let error = run_from_run_spec(&spec, &gz, dir.path())
            .err()
            .expect("completed-output reuse must reject a stray catalog file");

        assert!(error.to_string().contains("unexpected file"), "{error:#}");
        assert!(
            stray.exists(),
            "fail-closed verification must not mutate evidence"
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
    fn durable_cross_claim_validation_rejects_a_valid_contract_swapped_from_another_run() {
        let gz = gzip(SAMPLE_CSV);
        let first_spec = run_spec_for(&gz);
        let first_output = tempfile::tempdir().expect("first output dir");
        let first =
            run_from_run_spec(&first_spec, &gz, first_output.path()).expect("first operator run");
        let first_registry = VerifiedSourceBindingRegistry::from_run_spec(&first_spec)
            .expect("first source bindings");
        let (_, first_accepted) = accepted_dataset_for_run_spec_hash_with_registry(
            &first_spec,
            &first_spec.accepted_object.sha256,
            first_registry.registry(),
        )
        .expect("first accepted dataset");
        let first_fingerprint =
            conversion_fingerprint_for(&first_spec, &first_accepted, &first_registry)
                .expect("first conversion fingerprint");

        let versioned_object = |uri: String, role: &str| DurableObjectVersionIdentity {
            uri,
            sha256: sha256_hex(role.as_bytes()),
            byte_len: 1,
            version_id: format!("version-{role}"),
            e_tag: None,
        };
        let publication_receipt = versioned_object(
            portable_artifact_uri(&first_spec.manifest.output_prefix, "catalog-receipt.json"),
            "publication-receipt",
        );
        let terminal = DurableCompletionManifest::new(
            &first_spec,
            first_fingerprint.clone(),
            &first.batch_summary,
            publication_receipt.clone(),
            DurableCompletionArtifacts {
                result_contract: versioned_object(
                    portable_artifact_uri(&first_spec.manifest.output_prefix, RESULT_CONTRACT_FILE),
                    "result-contract",
                ),
                catalog_metadata: versioned_object(
                    portable_artifact_uri(
                        &first_spec.manifest.output_prefix,
                        CATALOG_METADATA_FILE,
                    ),
                    "catalog-metadata",
                ),
                published_catalog_proof: versioned_object(
                    portable_artifact_uri(
                        &first_spec.manifest.output_prefix,
                        PUBLISHED_CATALOG_PROOF_FILE,
                    ),
                    "published-catalog-proof",
                ),
                catalog_run_view_authority: versioned_object(
                    portable_artifact_uri(
                        &first_spec.manifest.output_prefix,
                        CATALOG_RUN_VIEW_AUTHORITY_FILE,
                    ),
                    "catalog-run-view-authority",
                ),
            },
        );
        let mut first_contract = first.output.contract.clone();
        first_contract.artifact_uris.nt_catalog_manifest_uri =
            Some(publication_receipt.uri.clone());
        validate_durable_result_contract_cross_claims(
            &first_contract,
            &first_spec,
            &first_fingerprint,
            &terminal,
        )
        .expect("matching durable cross-claims validate");

        let mut second_spec = first_spec.clone();
        second_spec.manifest.run_id = format!("{}-other", first_spec.manifest.run_id);
        second_spec.manifest.output_prefix = format!(
            "{}-other",
            first_spec.manifest.output_prefix.trim_end_matches('/')
        );
        let second_output = tempfile::tempdir().expect("second output dir");
        let second = run_from_run_spec(&second_spec, &gz, second_output.path())
            .expect("second operator run");
        let mut swapped_contract = second.output.contract.clone();
        swapped_contract.artifact_uris.nt_catalog_manifest_uri =
            Some(publication_receipt.uri.clone());

        let error = validate_durable_result_contract_cross_claims(
            &swapped_contract,
            &first_spec,
            &first_fingerprint,
            &terminal,
        )
        .expect_err("a valid contract from another run must fail terminal cross-claims");

        assert!(
            error.to_string().contains("submitted-run identity"),
            "{error:#}"
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
        let metadata: serde_json::Value =
            serde_json::from_str(COMMITTED_CATALOG_METADATA).expect("legacy metadata JSON parses");
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
        assert_eq!(metadata["metadata_version"], "catalog-metadata.v1");
        assert_eq!(metadata["direct_s3_catalog_access_proven"], false);
        assert!(
            serde_json::from_str::<ConversionCatalogMetadata>(COMMITTED_CATALOG_METADATA).is_err(),
            "current metadata schema must reject legacy direct-access fields"
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
