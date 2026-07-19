use futures_util::{StreamExt, stream::BoxStream};
use object_store::{
    Attributes, CopyOptions, GetOptions, GetResult, GetResultPayload, ListResult, MultipartUpload,
    ObjectMeta, ObjectStore, ObjectStoreExt, PutMode, PutMultipartOptions, PutOptions, PutPayload,
    PutResult, Result as ObjectStoreResult, memory::InMemory, path::Path as ObjectPath,
};
use std::{
    fmt, fs,
    io::Write,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
    time::Duration,
};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use crate::{
    artifact_store::{
        ArtifactIndexAuditEpoch, ArtifactIndexCommitPlan, ArtifactIndexCommitState,
        ArtifactIndexEvent, ArtifactIndexPointer, ArtifactIndexSnapshot, ArtifactIndexSnapshotRow,
        ArtifactIndexWriteAuthority, ArtifactIndexWriter, ArtifactKind, ArtifactLifecycleState,
        ArtifactLineageRef, ArtifactStorageProfile, ArtifactStoreConfig, CatalogCompression,
        CatalogDispatchConfig, CatalogEncodingConfig, CatalogProjectionBinding,
        CatalogProjectionPublicationObject, CatalogProjectionPublicationReceipt,
        CreateOnlyArtifactWriter, CreateOnlyProbeTranscript, CreateOnlyWriteDisposition,
        ResolvedArtifactRoot, S3_SINGLE_PUT_PROTOCOL_CEILING_BYTES, S3ArtifactStoreCredentials,
        StoredArtifactIndexPointer, hydrate_catalog_projection_from_receipt_guarded,
        is_terminal_create_indeterminate, persist_catalog_projection_for_source_binding_guarded,
    },
    backfill_execution_plan::BackfillExecutionWorkBudget,
    conversion_boundary::{
        CONVERSION_GENERATION_PATH_MARKER, CONVERSION_MANIFEST_FILE, CatalogConsumption,
        ConversionCatalogMetadata,
    },
    nt_catalog_capability::{
        NT_CATALOG_CAPABILITY_PROOF_SCHEMA_VERSION, NtCatalogCapabilityControls,
        NtCatalogCapabilityEvidence, NtCatalogCapabilityProof, NtCatalogCapabilityProofArtifact,
        NtCatalogCapabilityProofDocument, NtCatalogCapabilityRunSpec, NtCatalogCredentialSource,
        NtCatalogReadBackEvidence, SYNTHETIC_SOURCE_PROOF_ID,
    },
    operator::{
        CATALOG_DIR, DURABLE_COMPLETION_MANIFEST_FILE, DurableExecutionProvenance,
        DurableRunOutcome, OPERATOR_DURABLE_OUTPUT_CANDIDATE_SEAL_FILE, RunArtifacts, RunSpec,
        VerifiedSourceBindingRegistry,
        assert_current_durable_completion_absent_with_artifact_store_guarded,
        conversion_generation_sha256_for_run_spec, run_from_run_spec,
        run_from_run_spec_with_artifact_store_guarded,
    },
    operator_work_budget::{
        OperatorWorkBudget, OperatorWorkBudgetClock, OperatorWorkBudgetGuard,
        OperatorWorkBudgetStage,
    },
    result_contract::BacktestResultContract,
    run_manifest::{
        CATALOG_PROJECTION_MANIFEST_SCHEMA_VERSION, CATALOG_RUN_VIEW_AUTHORITY_FILE,
        CatalogProjectionManifestDocument, CatalogProjectionManifestObject,
        ManifestArtifactStoreSsmParameters, MarketStructureFixture,
    },
};
use flate2::{Compression, write::GzEncoder};
use serde::Deserialize;
use sha2::{Digest, Sha256};

const COMMITTED_RUN_SPEC: &str = include_str!(
    "../../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-run-spec.bnbusdc-2026-03-01.toml"
);
const COMMITTED_SOURCE_BINDINGS: &str = include_str!(
    "../../../specs/023-nt-research-analytics-platform/reference/backfill-source-bindings.v1.toml"
);
const CAPABILITY_PROOF_SPEC: &str = r#"
proof_run_id = "synthetic-capability-proof"
nt_revision = "6e059dcbb59ac1e582132fc431a581936c216c3c"
credential_source = "ssm"
proof_artifact_object_name = "nt-catalog-capability-proof.json"
expected_storage_options_keys = ["region"]
synthetic_fixture_coverage = ["binary-option", "perps-spot"]
synthetic_source_proof_id = "synthetic-fixture"
provenance = "synthetic"

[synthetic_fixtures.binary_option]
instrument_id = "RA001BINARY.POLYMARKET"
raw_symbol = "RA001BINARY"
asset_class = "ALTERNATIVE"
currency = "USDC"
activation_ns = 1700000000000000000
expiration_ns = 1700086400000000000
price_increment = "0.001"
size_increment = "0.01"

[synthetic_fixtures.perps_spot]
instrument_id = "RA001PERP.BYBIT"
raw_symbol = "RA001PERP"
base_currency = "BTC"
quote_currency = "USDC"
settlement_currency = "USDC"
is_inverse = false
price_increment = "0.1"
size_increment = "0.001"

[[synthetic_fixtures.trade_ticks]]
instrument_id = "RA001BINARY.POLYMARKET"
price = "0.500"
size = "1.00"
aggressor_side = "BUYER"
trade_id = "ra001-binary-0"
ts_event = 1700000000000000001
ts_init = 1700000000000000001

[[synthetic_fixtures.trade_ticks]]
instrument_id = "RA001PERP.BYBIT"
price = "50000.0"
size = "0.010"
aggressor_side = "SELLER"
trade_id = "ra001-perps-spot-0"
ts_event = 1700000000000000002
ts_init = 1700000000000000002

[ambient_credential_scrub]
unset_env_vars = [
  "AWS_ACCESS_KEY_ID",
  "AWS_SECRET_ACCESS_KEY",
  "AWS_SESSION_TOKEN",
  "AWS_DEFAULT_REGION",
  "AWS_REGION",
  "AWS_ENDPOINT",
  "AWS_ENDPOINT_URL_S3",
  "AWS_WEB_IDENTITY_TOKEN_FILE",
  "AWS_ROLE_ARN",
  "AWS_CONTAINER_CREDENTIALS_RELATIVE_URI",
  "AWS_CONTAINER_CREDENTIALS_FULL_URI",
  "AWS_PROFILE",
]
profile_file_paths_redirected = true
imds_blocked = true

[ssm_parameter_refs]
access_key_id = "/bolt-v2/research/catalog/aws-access-key-id"
secret_access_key = "/bolt-v2/research/catalog/aws-secret-access-key"
session_token = "/bolt-v2/research/catalog/aws-session-token"
"#;
const SAMPLE_CSV: &str = "id,timestamp,price,volume,side,rpi\n\
    1,1772323201665,617.2,0.3,buy,0\n\
    2,1772323312219,617.9,0.1456,sell,0\n\
    3,1772323312236,617,0.1544,sell,0\n";

struct ObservationExpiryClock {
    observations: AtomicUsize,
    expires_after_observation: usize,
}

#[derive(Default)]
struct ManualClock {
    now_millis: AtomicU64,
}

impl ManualClock {
    fn set(&self, now: Duration) {
        self.now_millis.store(
            u64::try_from(now.as_millis()).expect("test clock milliseconds fit u64"),
            Ordering::SeqCst,
        );
    }
}

impl OperatorWorkBudgetClock for ManualClock {
    fn now(&self) -> Duration {
        Duration::from_millis(self.now_millis.load(Ordering::SeqCst))
    }
}

impl OperatorWorkBudgetClock for ObservationExpiryClock {
    fn now(&self) -> Duration {
        if self.observations.fetch_add(1, Ordering::SeqCst) >= self.expires_after_observation {
            Duration::from_secs(1)
        } else {
            Duration::ZERO
        }
    }
}

fn one_second_guard(clock: Arc<ObservationExpiryClock>) -> OperatorWorkBudgetGuard {
    OperatorWorkBudgetGuard::with_clock(
        OperatorWorkBudget::Backfill(BackfillExecutionWorkBudget {
            max_decoded_bytes: u64::MAX,
            max_source_rows: 1,
            max_projected_row_groups: 10,
            max_wall_seconds: 1,
            require_object_selection_metadata: false,
        }),
        clock,
    )
    .expect("guard")
}

fn gzip(text: &str) -> Vec<u8> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(text.as_bytes()).expect("gzip write");
    encoder.finish().expect("gzip finish")
}

fn executed_durable_artifacts(outcome: DurableRunOutcome) -> Box<RunArtifacts> {
    outcome.into_artifacts()
}

async fn assert_store_uri_matches_file(
    store: &dyn ObjectStore,
    artifact_root: &ResolvedArtifactRoot,
    uri: &str,
    local_path: &Path,
) {
    let expected = fs::read(local_path).expect("local artifact bytes");
    let object_path = artifact_root
        .object_path_for_uri(uri)
        .expect("artifact URI under root");
    let stored = store
        .get(&object_path)
        .await
        .expect("durable contract artifact exists")
        .bytes()
        .await
        .expect("durable contract artifact bytes");
    assert_eq!(
        stored.as_ref(),
        expected.as_slice(),
        "durable artifact {uri} must match {}",
        local_path.display()
    );
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

fn assert_error_chain_contains(error: &anyhow::Error, expected: &str) {
    assert!(
        error
            .chain()
            .any(|cause| cause.to_string().contains(expected)),
        "expected error chain to contain {expected:?}, got {error:#}"
    );
}

fn catalog_physical_manifest(entries: &[(&str, &[u8])]) -> CatalogProjectionManifestDocument {
    let mut objects = entries
        .iter()
        .map(|(relative_path, payload)| CatalogProjectionManifestObject {
            relative_path: (*relative_path).to_string(),
            byte_len: u64::try_from(payload.len()).expect("catalog test payload length"),
            sha256: sha256_hex(payload),
        })
        .collect::<Vec<_>>();
    objects.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    CatalogProjectionManifestDocument {
        schema_version: CATALOG_PROJECTION_MANIFEST_SCHEMA_VERSION.to_string(),
        objects,
    }
}

#[derive(Debug, Clone)]
struct CommittedRunSpecFixture {
    spec: RunSpec,
    _source_bindings_dir: Arc<tempfile::TempDir>,
}

impl std::ops::Deref for CommittedRunSpecFixture {
    type Target = RunSpec;

    fn deref(&self) -> &Self::Target {
        &self.spec
    }
}

impl std::ops::DerefMut for CommittedRunSpecFixture {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.spec
    }
}

fn committed_run_spec_for(gz_bytes: &[u8]) -> CommittedRunSpecFixture {
    let mut spec: RunSpec = toml::from_str(COMMITTED_RUN_SPEC).expect("run-spec parses");
    spec.manifest.resolved_nt_version =
        crate::nt_dependency_proof::verified_nt_revision_from_embedded_manifests()
            .expect("BVS NautilusTrader dependency provenance");
    let source_bindings_dir = Arc::new(tempfile::tempdir().expect("source-bindings fixture dir"));
    let source_bindings_path = source_bindings_dir.path().join("source-bindings.toml");
    fs::write(&source_bindings_path, COMMITTED_SOURCE_BINDINGS)
        .expect("materialize committed source-bindings fixture");
    spec.source_bindings_path = source_bindings_path;
    let object_hash = sha256_hex(gz_bytes);
    spec.accepted_object.sha256 = object_hash.clone();
    spec.accepted_object.bytes = gz_bytes.len() as u64;
    spec.source_proof.raw_sample_hash = object_hash;
    bind_conversion_generation(&mut spec);
    CommittedRunSpecFixture {
        spec,
        _source_bindings_dir: source_bindings_dir,
    }
}

fn output_prefix_without_conversion_generation(output_prefix: &str) -> &str {
    output_prefix
        .rsplit_once(CONVERSION_GENERATION_PATH_MARKER)
        .filter(|(_, generation)| {
            generation.len() == 64
                && generation
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
        })
        .map_or(output_prefix, |(base, _)| base)
}

fn bind_conversion_generation(spec: &mut RunSpec) {
    let base =
        output_prefix_without_conversion_generation(&spec.manifest.output_prefix).to_string();
    let registry = VerifiedSourceBindingRegistry::from_run_spec(spec)
        .expect("snapshot source bindings for conversion generation");
    let generation = conversion_generation_sha256_for_run_spec(spec, &registry)
        .expect("derive conversion generation");
    spec.manifest.output_prefix = format!("{base}{CONVERSION_GENERATION_PATH_MARKER}{generation}");
}

fn artifact_config() -> ArtifactStoreConfig {
    toml::from_str(artifact_config_toml()).expect("artifact config parses")
}

#[derive(Debug, Deserialize)]
struct CommittedCapabilityProofFixture {
    artifact_store: ArtifactStoreConfig,
    nt_catalog_capability_proof: NtCatalogCapabilityRunSpec,
}

fn committed_capability_proof_fixture() -> CommittedCapabilityProofFixture {
    CommittedCapabilityProofFixture {
        artifact_store: artifact_config(),
        nt_catalog_capability_proof: toml::from_str(CAPABILITY_PROOF_SPEC)
            .expect("dedicated capability proof fixture parses"),
    }
}

fn successful_capability_evidence(
    root: &ResolvedArtifactRoot,
    proof: &NtCatalogCapabilityRunSpec,
) -> NtCatalogCapabilityEvidence {
    let probe_id = "capability-proof-test-probe";
    let catalog_uri = root
        .nt_catalog_synthetic_proof_root(&proof.proof_run_id)
        .expect("synthetic proof root");
    let binary_option_instrument_id = proof.synthetic_fixtures.binary_option.instrument_id.clone();
    let perps_spot_instrument_id = proof.synthetic_fixtures.perps_spot.instrument_id.clone();
    NtCatalogCapabilityEvidence {
        no_cloud_feature_gate_failed: true,
        ambient_credentials_scrubbed: true,
        invalid_credentials_write_failed: true,
        ssm_credentials_write_reopen_query_succeeded: true,
        nt_catalog_storage_option_keys: vec!["region".to_string()],
        read_back: NtCatalogReadBackEvidence {
            catalog_uri,
            query_files_succeeded: true,
            query_files_result_count: 1,
            write_instruments_succeeded: true,
            write_trade_ticks_succeeded: true,
            query_trade_ticks_succeeded: true,
            query_trade_ticks_result_count: 2,
            query_instruments_succeeded: true,
            query_instruments_result_count: 2,
            binary_option_instrument_read_back: true,
            binary_option_instrument_id,
            perps_spot_instrument_read_back: true,
            perps_spot_instrument_id,
        },
        create_only_probe: CreateOnlyProbeTranscript {
            probe_uri: root.create_only_probe_uri(probe_id),
            copy_source_uri: root.create_only_probe_copy_source_uri(probe_id),
            copy_dest_uri: root.create_only_probe_copy_dest_uri(probe_id),
            first_create_succeeded: true,
            duplicate_create_rejected: true,
            first_copy_succeeded: true,
            duplicate_copy_rejected: true,
        },
    }
}

fn synthetic_capability_proof(
    root: &ResolvedArtifactRoot,
    proof_run_id: &str,
    nt_revision: &str,
    storage_options_keys: Vec<String>,
) -> NtCatalogCapabilityProof {
    let proof = NtCatalogCapabilityProof {
        schema_version: NT_CATALOG_CAPABILITY_PROOF_SCHEMA_VERSION.to_string(),
        proof_run_id: proof_run_id.to_string(),
        nt_revision: nt_revision.to_string(),
        artifact_root_uri: root.artifact_root_uri().to_string(),
        synthetic_catalog_root_uri: root
            .nt_catalog_synthetic_proof_root(proof_run_id)
            .expect("synthetic proof root"),
        credential_source: NtCatalogCredentialSource::Ssm,
        storage_options_keys,
        synthetic_fixture_coverage: vec![
            MarketStructureFixture::BinaryOption,
            MarketStructureFixture::PerpsSpot,
        ],
        synthetic_source_proof_id: SYNTHETIC_SOURCE_PROOF_ID.to_string(),
        provenance: "synthetic".to_string(),
        controls: NtCatalogCapabilityControls {
            no_cloud_feature_gate_failed: true,
            ambient_credentials_scrubbed: true,
            invalid_credentials_write_failed: true,
            ssm_credentials_write_reopen_query_succeeded: true,
            conditional_put_probe_succeeded: true,
            copy_if_not_exists_probe_succeeded: true,
        },
    };
    proof.validate(root).expect("synthetic capability proof");
    proof
}

fn artifact_config_toml() -> &'static str {
    r#"
artifact_root = "s3://bolt-ra-artifacts/prod"
max_final_object_bytes = 1048576
catalog_projection_manifest_object = "catalog-projection-manifest.json"

[s3]
region = "us-east-1"
conditional_put = "etag"
copy_if_not_exists = "multipart"
terminal_commit_timeout_seconds = 60

[create_only_probe]
prefix = ".writer-probe"
object_name = "sentinel"
copy_source_object_name = "copy-source"
copy_dest_object_name = "copy-dest"

[subpaths]
raw = "raw"
nt_catalog = "nt-catalog"
nt_catalog_synthetic_proof = "nt-catalog-synthetic-proof"
source_proofs = "source-proofs"
backtests = "backtests"
artifact_index = "artifact-index"
research_analytics = "research-analytics"

[lifecycle]
retention = "forever"
default_delete_expiration = "disabled"
storage_profiles = ["active", "archive", "deep_archive"]

[lifecycle.quiet_window_seconds]
raw = 7200
nt_catalog = 7200
source_proofs = 7200
backtests = 3600
artifact_index = 0
research_analytics = 7200

[lifecycle.hot_index]
latest_pointer_storage_profile = "active"
current_snapshot_storage_profile = "active"
"#
}

#[test]
fn artifact_store_builds_s3_backend_with_required_capabilities() {
    let config = artifact_config();
    let credentials = S3ArtifactStoreCredentials::new(
        "configured-access-key".to_string(),
        "configured-secret-key".to_string(),
        Some("configured-session-token".to_string()),
    )
    .expect("test credentials are non-empty");
    let _store = config
        .build_s3_object_store_with_credentials(&credentials)
        .expect("S3 object store builder accepts required capability config");
}

#[test]
fn artifact_store_exposes_nt_catalog_storage_options_with_ssm_credentials() {
    let credentials = S3ArtifactStoreCredentials::new(
        "configured-access-key".to_string(),
        "configured-secret-key".to_string(),
        Some("configured-session-token".to_string()),
    )
    .expect("test credentials are non-empty");
    let options = artifact_config()
        .nt_catalog_storage_options_with_credentials(&credentials)
        .expect("NT catalog storage options include explicit SSM credentials");

    assert_eq!(options.get("region").map(String::as_str), Some("us-east-1"));
    assert_eq!(
        options.get("access_key_id").map(String::as_str),
        Some("configured-access-key")
    );
    assert_eq!(
        options.get("secret_access_key").map(String::as_str),
        Some("configured-secret-key")
    );
    assert_eq!(
        options.get("session_token").map(String::as_str),
        Some("configured-session-token")
    );
}

#[test]
fn s3_credentials_reject_blank_resolved_values() {
    let blank_access_key = match S3ArtifactStoreCredentials::new(
        String::from(" "),
        String::from("configured-secret-key"),
        Some(String::from("configured-session-token")),
    ) {
        Ok(_) => panic!("blank access key must be rejected"),
        Err(err) => err,
    };
    assert!(
        blank_access_key.to_string().contains("access_key_id"),
        "{blank_access_key}"
    );

    let blank_session_token = match S3ArtifactStoreCredentials::new(
        String::from("configured-access-key"),
        String::from("configured-secret-key"),
        Some(String::from(" ")),
    ) {
        Ok(_) => panic!("blank session token must be rejected"),
        Err(err) => err,
    };
    assert!(
        blank_session_token.to_string().contains("session_token"),
        "{blank_session_token}"
    );
}

#[test]
fn artifact_store_exposes_nt_catalog_storage_options_from_s3_config() {
    let options = artifact_config()
        .nt_catalog_storage_options()
        .expect("NT catalog storage options resolve from artifact-store config");

    assert_eq!(options.get("region").map(String::as_str), Some("us-east-1"));
}

#[test]
fn artifact_store_rejects_disabled_s3_conditional_put() {
    let disabled = artifact_config_toml().replace(
        "conditional_put = \"etag\"",
        "conditional_put = \"disabled\"",
    );
    let err = toml::from_str::<ArtifactStoreConfig>(&disabled)
        .expect_err("disabled conditional put must not parse as accepted artifact-store config");
    assert!(err.to_string().contains("conditional_put"), "{err}");
}

#[test]
fn artifact_store_requires_positive_sub_single_put_final_object_cap() {
    let missing = artifact_config_toml().replace("max_final_object_bytes = 1048576\n", "");
    let missing_error = toml::from_str::<ArtifactStoreConfig>(&missing)
        .expect_err("missing final-object byte cap must not parse");
    assert!(
        missing_error.to_string().contains("max_final_object_bytes"),
        "{missing_error}"
    );

    for invalid_cap in [0, S3_SINGLE_PUT_PROTOCOL_CEILING_BYTES] {
        let mut config = artifact_config();
        config.max_final_object_bytes = invalid_cap;
        let error = config
            .resolve()
            .expect_err("invalid final-object byte cap must fail closed");
        assert!(
            error.to_string().contains("max_final_object_bytes"),
            "{error}"
        );
    }

    let mut config = artifact_config();
    config.max_final_object_bytes = S3_SINGLE_PUT_PROTOCOL_CEILING_BYTES - 1;
    let root = config
        .resolve()
        .expect("largest permitted single-PUT cap resolves");
    assert_eq!(
        root.max_final_object_bytes(),
        S3_SINGLE_PUT_PROTOCOL_CEILING_BYTES - 1
    );
}

#[derive(Debug)]
struct NoListObjectStore {
    inner: InMemory,
}

impl NoListObjectStore {
    fn new() -> Self {
        Self {
            inner: InMemory::new(),
        }
    }
}

impl fmt::Display for NoListObjectStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("NoListObjectStore")
    }
}

#[async_trait::async_trait]
impl ObjectStore for NoListObjectStore {
    async fn put_opts(
        &self,
        location: &ObjectPath,
        payload: PutPayload,
        opts: PutOptions,
    ) -> ObjectStoreResult<PutResult> {
        self.inner.put_opts(location, payload, opts).await
    }

    async fn put_multipart_opts(
        &self,
        location: &ObjectPath,
        opts: PutMultipartOptions,
    ) -> ObjectStoreResult<Box<dyn MultipartUpload>> {
        self.inner.put_multipart_opts(location, opts).await
    }

    async fn get_opts(
        &self,
        location: &ObjectPath,
        options: GetOptions,
    ) -> ObjectStoreResult<GetResult> {
        self.inner.get_opts(location, options).await
    }

    fn delete_stream(
        &self,
        locations: BoxStream<'static, ObjectStoreResult<ObjectPath>>,
    ) -> BoxStream<'static, ObjectStoreResult<ObjectPath>> {
        self.inner.delete_stream(locations)
    }

    fn list(
        &self,
        _prefix: Option<&ObjectPath>,
    ) -> BoxStream<'static, ObjectStoreResult<ObjectMeta>> {
        panic!("artifact index normal discovery must not recursively list object storage")
    }

    fn list_with_offset(
        &self,
        _prefix: Option<&ObjectPath>,
        _offset: &ObjectPath,
    ) -> BoxStream<'static, ObjectStoreResult<ObjectMeta>> {
        panic!("artifact index normal discovery must not offset-list object storage")
    }

    async fn list_with_delimiter(
        &self,
        _prefix: Option<&ObjectPath>,
    ) -> ObjectStoreResult<ListResult> {
        panic!("artifact index normal discovery must not delimiter-list object storage")
    }

    async fn copy_opts(
        &self,
        from: &ObjectPath,
        to: &ObjectPath,
        options: CopyOptions,
    ) -> ObjectStoreResult<()> {
        self.inner.copy_opts(from, to, options).await
    }
}

#[derive(Debug)]
struct S3PreconditionOnCreateConflictStore {
    inner: InMemory,
    put_attempts: AtomicUsize,
    dishonest_get_path: Option<ObjectPath>,
    dishonest_get_size: Option<u64>,
    dishonest_get_range: Option<std::ops::Range<u64>>,
}

impl S3PreconditionOnCreateConflictStore {
    fn new() -> Self {
        Self {
            inner: InMemory::new(),
            put_attempts: AtomicUsize::new(0),
            dishonest_get_path: None,
            dishonest_get_size: None,
            dishonest_get_range: None,
        }
    }

    fn with_dishonest_conflict_metadata(path: ObjectPath, reported_size: u64) -> Self {
        Self {
            inner: InMemory::new(),
            put_attempts: AtomicUsize::new(0),
            dishonest_get_path: Some(path),
            dishonest_get_size: Some(reported_size),
            dishonest_get_range: None,
        }
    }

    fn with_dishonest_conflict_range(
        path: ObjectPath,
        reported_range: std::ops::Range<u64>,
    ) -> Self {
        Self {
            inner: InMemory::new(),
            put_attempts: AtomicUsize::new(0),
            dishonest_get_path: Some(path),
            dishonest_get_size: None,
            dishonest_get_range: Some(reported_range),
        }
    }

    fn put_attempts(&self) -> usize {
        self.put_attempts.load(Ordering::SeqCst)
    }
}

impl fmt::Display for S3PreconditionOnCreateConflictStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("S3PreconditionOnCreateConflictStore")
    }
}

async fn conflict_body_must_not_be_polled() -> ObjectStoreResult<bytes::Bytes> {
    panic!("conflict verifier polled body after metadata mismatch")
}

#[async_trait::async_trait]
impl ObjectStore for S3PreconditionOnCreateConflictStore {
    async fn put_opts(
        &self,
        location: &ObjectPath,
        payload: PutPayload,
        opts: PutOptions,
    ) -> ObjectStoreResult<PutResult> {
        self.put_attempts.fetch_add(1, Ordering::SeqCst);
        if matches!(opts.mode, PutMode::Create) && self.inner.head(location).await.is_ok() {
            return Err(object_store::Error::Precondition {
                path: location.to_string(),
                source: Box::new(std::io::Error::new(
                    std::io::ErrorKind::AlreadyExists,
                    "object already exists",
                )),
            });
        }
        self.inner.put_opts(location, payload, opts).await
    }

    async fn put_multipart_opts(
        &self,
        location: &ObjectPath,
        opts: PutMultipartOptions,
    ) -> ObjectStoreResult<Box<dyn MultipartUpload>> {
        self.inner.put_multipart_opts(location, opts).await
    }

    async fn get_opts(
        &self,
        location: &ObjectPath,
        options: GetOptions,
    ) -> ObjectStoreResult<GetResult> {
        let mut result = self.inner.get_opts(location, options).await?;
        if self.dishonest_get_path.as_ref() == Some(location) {
            if let Some(reported_size) = self.dishonest_get_size {
                result.meta.size = reported_size;
            }
            if let Some(reported_range) = &self.dishonest_get_range {
                result.range = reported_range.clone();
            }
            let body: BoxStream<'static, ObjectStoreResult<bytes::Bytes>> =
                futures_util::stream::once(conflict_body_must_not_be_polled()).boxed();
            result.payload = GetResultPayload::Stream(body);
        }
        Ok(result)
    }

    fn delete_stream(
        &self,
        locations: BoxStream<'static, ObjectStoreResult<ObjectPath>>,
    ) -> BoxStream<'static, ObjectStoreResult<ObjectPath>> {
        self.inner.delete_stream(locations)
    }

    fn list(
        &self,
        prefix: Option<&ObjectPath>,
    ) -> BoxStream<'static, ObjectStoreResult<ObjectMeta>> {
        self.inner.list(prefix)
    }

    fn list_with_offset(
        &self,
        prefix: Option<&ObjectPath>,
        offset: &ObjectPath,
    ) -> BoxStream<'static, ObjectStoreResult<ObjectMeta>> {
        self.inner.list_with_offset(prefix, offset)
    }

    async fn list_with_delimiter(
        &self,
        prefix: Option<&ObjectPath>,
    ) -> ObjectStoreResult<ListResult> {
        self.inner.list_with_delimiter(prefix).await
    }

    async fn copy_opts(
        &self,
        from: &ObjectPath,
        to: &ObjectPath,
        options: CopyOptions,
    ) -> ObjectStoreResult<()> {
        self.inner.copy_opts(from, to, options).await
    }
}

type CatalogPutHook = Arc<dyn Fn(&ObjectPath, usize) + Send + Sync>;
type CatalogGetHook = Arc<dyn Fn(&ObjectPath, usize) + Send + Sync>;

#[derive(Clone)]
struct VersionedCatalogObject {
    location: ObjectPath,
    version_id: String,
    bytes: bytes::Bytes,
    meta: ObjectMeta,
}

#[derive(Default)]
struct VersionedCatalogState {
    versions: Vec<VersionedCatalogObject>,
    current: Vec<(ObjectPath, String)>,
}

struct VersionedCatalogStore {
    inner: InMemory,
    state: Mutex<VersionedCatalogState>,
    put_attempts: Mutex<Vec<ObjectPath>>,
    successful_puts: AtomicUsize,
    successful_gets: AtomicUsize,
    omit_version_for: Mutex<Vec<ObjectPath>>,
    empty_version_for: Mutex<Vec<ObjectPath>>,
    null_version_for: Mutex<Vec<ObjectPath>>,
    omit_put_e_tag_for: Mutex<Vec<ObjectPath>>,
    blank_put_e_tag_for: Mutex<Vec<ObjectPath>>,
    omit_get_version_for: Mutex<Vec<ObjectPath>>,
    omit_current_e_tag_once_for: Mutex<Vec<ObjectPath>>,
    blank_current_e_tag_once_for: Mutex<Vec<ObjectPath>>,
    omit_exact_get_e_tag_once_for: Mutex<Vec<ObjectPath>>,
    replace_exact_get_e_tag_once_for: Mutex<Vec<(ObjectPath, String)>>,
    lost_put_ack_for: Mutex<Vec<ObjectPath>>,
    pending_put_for: Mutex<Vec<ObjectPath>>,
    fail_exact_version_get_for: Mutex<Vec<ObjectPath>>,
    exact_version_get_attempts: Mutex<Vec<(ObjectPath, String, Option<String>)>>,
    after_successful_put: Option<CatalogPutHook>,
    after_successful_get: Option<CatalogGetHook>,
}

impl VersionedCatalogStore {
    fn new() -> Self {
        Self {
            inner: InMemory::new(),
            state: Mutex::new(VersionedCatalogState::default()),
            put_attempts: Mutex::new(Vec::new()),
            successful_puts: AtomicUsize::new(0),
            successful_gets: AtomicUsize::new(0),
            omit_version_for: Mutex::new(Vec::new()),
            empty_version_for: Mutex::new(Vec::new()),
            null_version_for: Mutex::new(Vec::new()),
            omit_put_e_tag_for: Mutex::new(Vec::new()),
            blank_put_e_tag_for: Mutex::new(Vec::new()),
            omit_get_version_for: Mutex::new(Vec::new()),
            omit_current_e_tag_once_for: Mutex::new(Vec::new()),
            blank_current_e_tag_once_for: Mutex::new(Vec::new()),
            omit_exact_get_e_tag_once_for: Mutex::new(Vec::new()),
            replace_exact_get_e_tag_once_for: Mutex::new(Vec::new()),
            lost_put_ack_for: Mutex::new(Vec::new()),
            pending_put_for: Mutex::new(Vec::new()),
            fail_exact_version_get_for: Mutex::new(Vec::new()),
            exact_version_get_attempts: Mutex::new(Vec::new()),
            after_successful_put: None,
            after_successful_get: None,
        }
    }

    fn with_omitted_version(self, path: ObjectPath) -> Self {
        self.omit_version_for
            .lock()
            .expect("omitted version lock")
            .push(path);
        self
    }

    fn omit_version(&self, path: ObjectPath) {
        self.omit_get_version_for
            .lock()
            .expect("omitted GET version lock")
            .push(path);
    }

    fn omit_current_e_tag_once(&self, path: ObjectPath) {
        self.omit_current_e_tag_once_for
            .lock()
            .expect("omitted current ETag lock")
            .push(path);
    }

    fn blank_current_e_tag_once(&self, path: ObjectPath) {
        self.blank_current_e_tag_once_for
            .lock()
            .expect("blank current ETag lock")
            .push(path);
    }

    fn omit_exact_get_e_tag_once(&self, path: ObjectPath) {
        self.omit_exact_get_e_tag_once_for
            .lock()
            .expect("omitted exact GET ETag lock")
            .push(path);
    }

    fn replace_exact_get_e_tag_once(&self, path: ObjectPath, e_tag: String) {
        self.replace_exact_get_e_tag_once_for
            .lock()
            .expect("replaced exact GET ETag lock")
            .push((path, e_tag));
    }

    fn with_empty_version(self, path: ObjectPath) -> Self {
        self.empty_version_for
            .lock()
            .expect("empty version lock")
            .push(path);
        self
    }

    fn with_omitted_put_e_tag(self, path: ObjectPath) -> Self {
        self.omit_put_e_tag_for
            .lock()
            .expect("omitted PUT ETag lock")
            .push(path);
        self
    }

    fn with_blank_put_e_tag(self, path: ObjectPath) -> Self {
        self.blank_put_e_tag_for
            .lock()
            .expect("blank PUT ETag lock")
            .push(path);
        self
    }

    fn with_null_version(self, path: ObjectPath) -> Self {
        self.null_version_for
            .lock()
            .expect("null version lock")
            .push(path);
        self
    }

    fn with_lost_put_ack(self, path: ObjectPath) -> Self {
        self.lost_put_ack_for
            .lock()
            .expect("lost put acknowledgement lock")
            .push(path);
        self
    }

    fn with_pending_put(self, path: ObjectPath) -> Self {
        self.pending_put_for
            .lock()
            .expect("pending PUT lock")
            .push(path);
        self
    }

    fn fail_exact_version_get(&self, path: ObjectPath) {
        self.fail_exact_version_get_for
            .lock()
            .expect("failed exact-version GET lock")
            .push(path);
    }

    fn with_after_successful_put(mut self, hook: CatalogPutHook) -> Self {
        self.after_successful_put = Some(hook);
        self
    }

    fn with_after_successful_get(mut self, hook: CatalogGetHook) -> Self {
        self.after_successful_get = Some(hook);
        self
    }

    fn put_attempts(&self) -> Vec<ObjectPath> {
        self.put_attempts.lock().expect("put attempts lock").clone()
    }

    fn omits_version(&self, path: &ObjectPath) -> bool {
        self.omit_version_for
            .lock()
            .expect("omitted version lock")
            .iter()
            .any(|candidate| candidate == path)
    }

    fn returns_empty_version(&self, path: &ObjectPath) -> bool {
        self.empty_version_for
            .lock()
            .expect("empty version lock")
            .iter()
            .any(|candidate| candidate == path)
    }

    fn returns_null_version(&self, path: &ObjectPath) -> bool {
        self.null_version_for
            .lock()
            .expect("null version lock")
            .iter()
            .any(|candidate| candidate == path)
    }

    fn omits_put_e_tag(&self, path: &ObjectPath) -> bool {
        self.omit_put_e_tag_for
            .lock()
            .expect("omitted PUT ETag lock")
            .iter()
            .any(|candidate| candidate == path)
    }

    fn returns_blank_put_e_tag(&self, path: &ObjectPath) -> bool {
        self.blank_put_e_tag_for
            .lock()
            .expect("blank PUT ETag lock")
            .iter()
            .any(|candidate| candidate == path)
    }

    fn omits_get_version(&self, path: &ObjectPath) -> bool {
        self.omit_get_version_for
            .lock()
            .expect("omitted GET version lock")
            .iter()
            .any(|candidate| candidate == path)
    }

    fn take_omitted_current_e_tag(&self, path: &ObjectPath) -> bool {
        let mut paths = self
            .omit_current_e_tag_once_for
            .lock()
            .expect("omitted current ETag lock");
        let Some(index) = paths.iter().position(|candidate| candidate == path) else {
            return false;
        };
        paths.remove(index);
        true
    }

    fn take_blank_current_e_tag(&self, path: &ObjectPath) -> bool {
        let mut paths = self
            .blank_current_e_tag_once_for
            .lock()
            .expect("blank current ETag lock");
        let Some(index) = paths.iter().position(|candidate| candidate == path) else {
            return false;
        };
        paths.remove(index);
        true
    }

    fn take_omitted_exact_get_e_tag(&self, path: &ObjectPath) -> bool {
        let mut paths = self
            .omit_exact_get_e_tag_once_for
            .lock()
            .expect("omitted exact GET ETag lock");
        let Some(index) = paths.iter().position(|candidate| candidate == path) else {
            return false;
        };
        paths.remove(index);
        true
    }

    fn take_replaced_exact_get_e_tag(&self, path: &ObjectPath) -> Option<String> {
        let mut replacements = self
            .replace_exact_get_e_tag_once_for
            .lock()
            .expect("replaced exact GET ETag lock");
        let index = replacements
            .iter()
            .position(|(candidate, _)| candidate == path)?;
        Some(replacements.remove(index).1)
    }

    fn loses_put_ack(&self, path: &ObjectPath) -> bool {
        self.lost_put_ack_for
            .lock()
            .expect("lost put acknowledgement lock")
            .iter()
            .any(|candidate| candidate == path)
    }

    fn keeps_put_pending(&self, path: &ObjectPath) -> bool {
        self.pending_put_for
            .lock()
            .expect("pending PUT lock")
            .iter()
            .any(|candidate| candidate == path)
    }

    fn fails_exact_version_get(&self, path: &ObjectPath) -> bool {
        self.fail_exact_version_get_for
            .lock()
            .expect("failed exact-version GET lock")
            .iter()
            .any(|candidate| candidate == path)
    }

    fn exact_version_get_attempts(&self) -> Vec<(ObjectPath, String, Option<String>)> {
        self.exact_version_get_attempts
            .lock()
            .expect("exact version GET attempts lock")
            .clone()
    }

    fn recorded_version(&self, path: &ObjectPath) -> Option<(String, Option<String>)> {
        let state = self.state.lock().expect("catalog version state lock");
        let version_id = state
            .current
            .iter()
            .find(|(candidate, _)| candidate == path)
            .map(|(_, version_id)| version_id)?;
        state
            .versions
            .iter()
            .find(|object| object.location == *path && object.version_id == *version_id)
            .map(|object| (object.version_id.clone(), object.meta.e_tag.clone()))
    }

    fn recorded_object_version(
        &self,
        path: &ObjectPath,
        version_id: &str,
    ) -> Option<VersionedCatalogObject> {
        self.state
            .lock()
            .expect("catalog version state lock")
            .versions
            .iter()
            .find(|object| object.location == *path && object.version_id == version_id)
            .cloned()
    }

    async fn record_current_version(
        &self,
        location: &ObjectPath,
        version_id: String,
        e_tag: Option<String>,
    ) -> ObjectStoreResult<()> {
        let current = self.inner.get(location).await?;
        let mut meta = current.meta.clone();
        let bytes = current.bytes().await?;
        meta.version = Some(version_id.clone());
        if e_tag.is_some() {
            meta.e_tag = e_tag;
        }
        let mut state = self.state.lock().expect("catalog version state lock");
        state.versions.push(VersionedCatalogObject {
            location: location.clone(),
            version_id: version_id.clone(),
            bytes,
            meta,
        });
        if let Some((_, current_version)) = state
            .current
            .iter_mut()
            .find(|(candidate, _)| candidate == location)
        {
            *current_version = version_id;
        } else {
            state.current.push((location.clone(), version_id));
        }
        Ok(())
    }
}

impl fmt::Debug for VersionedCatalogStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VersionedCatalogStore")
            .finish_non_exhaustive()
    }
}

impl fmt::Display for VersionedCatalogStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("VersionedCatalogStore")
    }
}

#[async_trait::async_trait]
impl ObjectStore for VersionedCatalogStore {
    async fn put_opts(
        &self,
        location: &ObjectPath,
        payload: PutPayload,
        opts: PutOptions,
    ) -> ObjectStoreResult<PutResult> {
        self.put_attempts
            .lock()
            .expect("put attempts lock")
            .push(location.clone());
        let mut result = self.inner.put_opts(location, payload, opts).await?;
        let sequence = self.successful_puts.fetch_add(1, Ordering::SeqCst) + 1;
        let version_id = format!("catalog-version-{sequence}");
        let e_tag = result.e_tag.clone();
        self.record_current_version(location, version_id.clone(), e_tag)
            .await?;
        if self.returns_null_version(location) {
            result.version = Some("null".to_string());
        } else if self.returns_empty_version(location) {
            result.version = Some(String::new());
        } else if !self.omits_version(location) {
            result.version = Some(version_id);
        }
        if self.omits_put_e_tag(location) {
            result.e_tag = None;
        } else if self.returns_blank_put_e_tag(location) {
            result.e_tag = Some("   ".to_string());
        }
        if let Some(hook) = &self.after_successful_put {
            hook(location, sequence);
        }
        if self.loses_put_ack(location) {
            return Err(object_store::Error::Generic {
                store: "VersionedCatalogStore",
                source: Box::new(std::io::Error::new(
                    std::io::ErrorKind::ConnectionAborted,
                    "synthetic lost PutObject acknowledgement",
                )),
            });
        }
        if self.keeps_put_pending(location) {
            return std::future::pending::<ObjectStoreResult<PutResult>>().await;
        }
        Ok(result)
    }

    async fn put_multipart_opts(
        &self,
        location: &ObjectPath,
        opts: PutMultipartOptions,
    ) -> ObjectStoreResult<Box<dyn MultipartUpload>> {
        self.inner.put_multipart_opts(location, opts).await
    }

    async fn get_opts(
        &self,
        location: &ObjectPath,
        options: GetOptions,
    ) -> ObjectStoreResult<GetResult> {
        if let Some(requested_version) = options.version.as_ref() {
            self.exact_version_get_attempts
                .lock()
                .expect("exact version GET attempts lock")
                .push((
                    location.clone(),
                    requested_version.clone(),
                    options.if_match.clone(),
                ));
            if self.fails_exact_version_get(location) {
                return Err(object_store::Error::Generic {
                    store: "VersionedCatalogStore",
                    source: Box::new(std::io::Error::new(
                        std::io::ErrorKind::ConnectionAborted,
                        "synthetic exact-version GET failure",
                    )),
                });
            }
            let Some(mut object) = self.recorded_object_version(location, requested_version) else {
                return Err(object_store::Error::NotFound {
                    path: location.to_string(),
                    source: Box::new(std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        "requested object version does not exist",
                    )),
                });
            };
            if let Some(if_match) = options.if_match.as_ref()
                && object.meta.e_tag.as_ref() != Some(if_match)
            {
                return Err(object_store::Error::Precondition {
                    path: location.to_string(),
                    source: Box::new(std::io::Error::other(
                        "requested exact-version ETag does not match",
                    )),
                });
            }
            if self.omits_get_version(location) {
                object.meta.version = None;
            }
            if self.take_omitted_exact_get_e_tag(location) {
                object.meta.e_tag = None;
            } else if let Some(e_tag) = self.take_replaced_exact_get_e_tag(location) {
                object.meta.e_tag = Some(e_tag);
            }
            let range = 0..object.meta.size;
            let payload =
                futures_util::stream::once(
                    async move { Ok::<_, object_store::Error>(object.bytes) },
                )
                .boxed();
            let result = GetResult {
                payload: GetResultPayload::Stream(payload),
                meta: object.meta,
                range,
                attributes: Attributes::default(),
            };
            let sequence = self.successful_gets.fetch_add(1, Ordering::SeqCst) + 1;
            if let Some(hook) = &self.after_successful_get {
                hook(location, sequence);
            }
            return Ok(result);
        }

        let is_head = options.head;
        let mut result = self.inner.get_opts(location, options).await?;
        if let Some((version_id, e_tag)) = self.recorded_version(location) {
            result.meta.version = (!self.omits_get_version(location)).then_some(version_id);
            if e_tag.is_some() {
                result.meta.e_tag = e_tag;
            }
        }
        if is_head && self.take_omitted_current_e_tag(location) {
            result.meta.e_tag = None;
        } else if is_head && self.take_blank_current_e_tag(location) {
            result.meta.e_tag = Some("   ".to_string());
        }
        let sequence = self.successful_gets.fetch_add(1, Ordering::SeqCst) + 1;
        if let Some(hook) = &self.after_successful_get {
            hook(location, sequence);
        }
        Ok(result)
    }

    fn delete_stream(
        &self,
        locations: BoxStream<'static, ObjectStoreResult<ObjectPath>>,
    ) -> BoxStream<'static, ObjectStoreResult<ObjectPath>> {
        self.inner.delete_stream(locations)
    }

    fn list(
        &self,
        prefix: Option<&ObjectPath>,
    ) -> BoxStream<'static, ObjectStoreResult<ObjectMeta>> {
        self.inner.list(prefix)
    }

    fn list_with_offset(
        &self,
        prefix: Option<&ObjectPath>,
        offset: &ObjectPath,
    ) -> BoxStream<'static, ObjectStoreResult<ObjectMeta>> {
        self.inner.list_with_offset(prefix, offset)
    }

    async fn list_with_delimiter(
        &self,
        prefix: Option<&ObjectPath>,
    ) -> ObjectStoreResult<ListResult> {
        self.inner.list_with_delimiter(prefix).await
    }

    async fn copy_opts(
        &self,
        from: &ObjectPath,
        to: &ObjectPath,
        options: CopyOptions,
    ) -> ObjectStoreResult<()> {
        self.inner.copy_opts(from, to, options).await
    }
}

#[test]
fn resolves_nt_catalog_projection_root_from_single_toml_artifact_root() {
    let root = artifact_config().resolve().expect("valid artifact root");

    assert_eq!(
        root.nt_catalog_projection_root("projection-run-123"),
        "s3://bolt-ra-artifacts/prod/nt-catalog/v1/projection=projection-run-123/"
    );
    assert_eq!(
        root.backtest_run_root(MarketStructureFixture::PerpsSpot, "run-123"),
        "s3://bolt-ra-artifacts/prod/backtests/v1/fixture=perps-spot/run=run-123/"
    );
    assert_eq!(
        root.latest_pointer(ArtifactKind::Backtests),
        "s3://bolt-ra-artifacts/prod/artifact-index/v1/pointers/kind=backtests/latest.json"
    );
}

#[test]
fn resolves_synthetic_nt_catalog_proof_root_outside_canonical_catalog() {
    let root = artifact_config().resolve().expect("valid artifact root");

    let synthetic = root
        .nt_catalog_synthetic_proof_root("s3-proof-run-123")
        .expect("valid synthetic proof root");

    assert_eq!(
        synthetic,
        "s3://bolt-ra-artifacts/prod/nt-catalog-synthetic-proof/v1/proof=s3-proof-run-123/"
    );
    let canonical_catalog_root = root.typed_root(ArtifactKind::NtCatalog);
    assert!(
        !synthetic.starts_with(canonical_catalog_root.as_str()),
        "{synthetic}"
    );
    assert!(
        !synthetic.contains("/nt-catalog/v1/"),
        "synthetic proof root must be disjoint from canonical catalog roots: {synthetic}"
    );
    assert!(
        root.nt_catalog_synthetic_proof_root("bad/proof").is_err(),
        "synthetic proof ids must be path tokens"
    );
}

#[tokio::test]
async fn put_conflict_rejects_dishonest_existing_size_before_polling_body() {
    let root = artifact_config().resolve().expect("valid artifact root");
    let probe_id = "dishonest-create-conflict-size";
    let path = root
        .object_path_for_uri(&root.create_only_probe_uri(probe_id))
        .expect("probe path under artifact root");
    let reported_size = u64::try_from(probe_id.len()).expect("payload size") + 1;
    let store =
        S3PreconditionOnCreateConflictStore::with_dishonest_conflict_metadata(path, reported_size);
    let writer = CreateOnlyArtifactWriter::new(&store, &root);

    let error = writer
        .probe_create_only_guarded(&root, probe_id, &OperatorWorkBudgetGuard::unbounded())
        .await
        .expect_err("dishonest conflict metadata must fail closed");

    assert!(error.to_string().contains("Content-Length"), "{error:#}");
    assert_eq!(store.put_attempts(), 2);
}

#[tokio::test]
async fn put_conflict_rejects_dishonest_existing_range_before_polling_body() {
    let root = artifact_config().resolve().expect("valid artifact root");
    let probe_id = "dishonest-create-conflict-range";
    let path = root
        .object_path_for_uri(&root.create_only_probe_uri(probe_id))
        .expect("probe path under artifact root");
    let payload_size = u64::try_from(probe_id.len()).expect("payload size");
    let store =
        S3PreconditionOnCreateConflictStore::with_dishonest_conflict_range(path, 1..payload_size);
    let writer = CreateOnlyArtifactWriter::new(&store, &root);

    let error = writer
        .probe_create_only_guarded(&root, probe_id, &OperatorWorkBudgetGuard::unbounded())
        .await
        .expect_err("dishonest conflict range must fail closed");

    assert!(error.to_string().contains("response range"), "{error:#}");
    assert_eq!(store.put_attempts(), 2);
}

#[tokio::test]
async fn copy_conflict_rejects_dishonest_existing_size_before_polling_body() {
    let root = artifact_config().resolve().expect("valid artifact root");
    let probe_id = "dishonest-copy-conflict";
    let destination_uri = root.create_only_probe_copy_dest_uri(probe_id);
    let destination_path = root
        .object_path_for_uri(&destination_uri)
        .expect("copy destination under artifact root");
    let reported_size = u64::try_from(probe_id.len()).expect("probe id size") + 1;
    let store = S3PreconditionOnCreateConflictStore::with_dishonest_conflict_metadata(
        destination_path,
        reported_size,
    );
    let writer = CreateOnlyArtifactWriter::new(&store, &root);

    let error = writer
        .probe_create_only_guarded(&root, probe_id, &OperatorWorkBudgetGuard::unbounded())
        .await
        .expect_err("dishonest copy-conflict metadata must fail closed");

    assert!(error.to_string().contains("Content-Length"), "{error:#}");
}

#[tokio::test]
async fn nt_catalog_capability_proof_requires_synthetic_ssm_direct_s3_controls() {
    let fixture = committed_capability_proof_fixture();
    let plan = fixture
        .nt_catalog_capability_proof
        .proof_plan(&fixture.artifact_store)
        .expect("committed capability proof plan");
    assert_eq!(plan.credential_source, NtCatalogCredentialSource::Ssm);
    assert_eq!(plan.storage_options_keys, vec!["region".to_string()]);
    assert!(plan.ambient_credential_scrub.profile_file_paths_redirected);
    assert!(plan.ambient_credential_scrub.imds_blocked);
    assert!(
        plan.synthetic_catalog_root_uri
            .contains("/nt-catalog-synthetic-proof/v1/proof=synthetic-capability-proof/")
    );
    assert!(
        !plan.synthetic_catalog_root_uri.contains("/nt-catalog/v1/"),
        "capability proof plan must not point at the canonical NT catalog root"
    );

    let committed_root = fixture
        .artifact_store
        .resolve()
        .expect("committed artifact root");
    let evidence =
        successful_capability_evidence(&committed_root, &fixture.nt_catalog_capability_proof);
    assert_eq!(
        evidence.read_back.catalog_uri,
        plan.synthetic_catalog_root_uri
    );
    let committed_proof = fixture
        .nt_catalog_capability_proof
        .completed_proof_from_evidence(&fixture.artifact_store, &evidence)
        .expect("committed capability proof completes from evidence");
    committed_proof
        .direct_s3_catalog_access_proven(&committed_root)
        .expect("committed proof controls validate");
    let mut mismatched_storage_options_evidence = evidence.clone();
    mismatched_storage_options_evidence.nt_catalog_storage_option_keys =
        vec!["endpoint_url".to_string()];
    assert!(
        fixture
            .nt_catalog_capability_proof
            .completed_proof_from_evidence(
                &fixture.artifact_store,
                &mismatched_storage_options_evidence
            )
            .is_err(),
        "capability proof must reject NT storage option evidence that differs from the plan"
    );
    let mut mismatched_read_back_catalog_uri_evidence = evidence.clone();
    mismatched_read_back_catalog_uri_evidence
        .read_back
        .catalog_uri = committed_root.nt_catalog_projection_root("canonical-projection");
    assert!(
        fixture
            .nt_catalog_capability_proof
            .completed_proof_from_evidence(
                &fixture.artifact_store,
                &mismatched_read_back_catalog_uri_evidence
            )
            .is_err(),
        "capability proof must reject read-back evidence from outside the synthetic catalog root"
    );
    let mut missing_query_evidence = evidence.clone();
    missing_query_evidence.read_back.query_instruments_succeeded = false;
    assert!(
        fixture
            .nt_catalog_capability_proof
            .completed_proof_from_evidence(&fixture.artifact_store, &missing_query_evidence)
            .is_err(),
        "capability proof must reject missing NT query evidence"
    );
    let mut missing_query_count_evidence = evidence.clone();
    missing_query_count_evidence
        .read_back
        .query_files_result_count = 0;
    assert!(
        fixture
            .nt_catalog_capability_proof
            .completed_proof_from_evidence(&fixture.artifact_store, &missing_query_count_evidence)
            .is_err(),
        "capability proof must reject missing NT query_files result count evidence"
    );
    let mut missing_trade_query_count_evidence = evidence.clone();
    missing_trade_query_count_evidence
        .read_back
        .query_trade_ticks_result_count = 0;
    assert!(
        fixture
            .nt_catalog_capability_proof
            .completed_proof_from_evidence(
                &fixture.artifact_store,
                &missing_trade_query_count_evidence
            )
            .is_err(),
        "capability proof must reject missing NT query_typed_data result count evidence"
    );
    let mut missing_instrument_id_evidence = evidence.clone();
    missing_instrument_id_evidence
        .read_back
        .binary_option_instrument_id
        .clear();
    assert!(
        fixture
            .nt_catalog_capability_proof
            .completed_proof_from_evidence(&fixture.artifact_store, &missing_instrument_id_evidence)
            .is_err(),
        "capability proof must reject missing binary-option read-back instrument id"
    );
    let mut missing_duplicate_copy_evidence = evidence.clone();
    missing_duplicate_copy_evidence
        .create_only_probe
        .duplicate_copy_rejected = false;
    assert!(
        fixture
            .nt_catalog_capability_proof
            .completed_proof_from_evidence(
                &fixture.artifact_store,
                &missing_duplicate_copy_evidence
            )
            .is_err(),
        "capability proof must reject missing duplicate copy-if-not-exists evidence"
    );
    let persisted_path = committed_root
        .object_path_for_uri(&plan.proof_artifact_uri)
        .expect("proof artifact uri is under artifact root");
    let lost_ack_store = VersionedCatalogStore::new().with_lost_put_ack(persisted_path.clone());
    let lost_ack_writer = CreateOnlyArtifactWriter::new(&lost_ack_store, &committed_root);
    let lost_ack_error = fixture
        .nt_catalog_capability_proof
        .persist_completed_proof_from_evidence_guarded(
            &fixture.artifact_store,
            &lost_ack_writer,
            &evidence,
            &OperatorWorkBudgetGuard::unbounded(),
        )
        .await
        .expect_err("lost proof acknowledgement must stop as indeterminate");
    assert!(
        is_terminal_create_indeterminate(&lost_ack_error),
        "{lost_ack_error:#}"
    );
    assert!(
        lost_ack_store
            .exact_version_get_attempts()
            .iter()
            .all(|(path, _, _)| path != &persisted_path),
        "proof publication must not recover an indeterminate acknowledgement"
    );

    let store = VersionedCatalogStore::new();
    let writer = CreateOnlyArtifactWriter::new(&store, &committed_root);
    let persisted = fixture
        .nt_catalog_capability_proof
        .persist_completed_proof_from_evidence_guarded(
            &fixture.artifact_store,
            &writer,
            &evidence,
            &OperatorWorkBudgetGuard::unbounded(),
        )
        .await
        .expect("proof artifact persists create-only");
    assert!(persisted.proof_artifact_uri.ends_with(
        "/nt-catalog-synthetic-proof/v1/proof=synthetic-capability-proof/nt-catalog-capability-proof.json"
    ));
    assert_eq!(persisted.proof_artifact_sha256.len(), 64);
    assert_eq!(
        persisted.proof_artifact_create_only_write,
        CreateOnlyWriteDisposition::Created
    );
    let (persisted_version_id, persisted_e_tag) = store
        .recorded_version(&persisted_path)
        .expect("capability proof exact version");
    let persisted_e_tag = persisted_e_tag.expect("capability proof ETag");
    assert_eq!(persisted.proof_artifact_version_id, persisted_version_id);
    assert_eq!(persisted.proof_artifact_e_tag, persisted_e_tag);
    let serialized_persisted =
        serde_json::to_value(&persisted).expect("serialize persisted capability proof artifact");
    let mut missing_e_tag = serialized_persisted.clone();
    missing_e_tag
        .as_object_mut()
        .expect("capability proof artifact object")
        .remove("proof_artifact_e_tag");
    let error = serde_json::from_value::<NtCatalogCapabilityProofArtifact>(missing_e_tag)
        .expect_err("missing capability proof artifact ETag must fail deserialization");
    assert!(
        error.to_string().contains("proof_artifact_e_tag"),
        "{error:#}"
    );
    let mut blank_e_tag = serialized_persisted;
    blank_e_tag["proof_artifact_e_tag"] = serde_json::Value::String("   ".to_string());
    let error = serde_json::from_value::<NtCatalogCapabilityProofArtifact>(blank_e_tag)
        .expect_err("blank capability proof artifact ETag must fail deserialization");
    assert!(error.to_string().contains("ETag"), "{error:#}");
    persisted
        .proof
        .direct_s3_catalog_access_proven(&committed_root)
        .expect("persisted proof validates");
    assert_eq!(
        store
            .exact_version_get_attempts()
            .iter()
            .filter(|(path, _, _)| path == &persisted_path)
            .count(),
        0,
        "fresh capability proof publication must accept only its direct create acknowledgement"
    );
    let persisted_bytes = store
        .get(&persisted_path)
        .await
        .expect("proof artifact is readable")
        .bytes()
        .await
        .expect("proof artifact bytes");
    assert_eq!(
        sha256_hex(persisted_bytes.as_ref()),
        persisted.proof_artifact_sha256
    );
    let persisted_document =
        serde_json::from_slice::<NtCatalogCapabilityProofDocument>(persisted_bytes.as_ref())
            .expect("proof artifact stores proof document");
    assert_eq!(persisted_document.proof, persisted.proof);
    assert_eq!(persisted_document.evidence, persisted.evidence);
    assert_eq!(persisted_document.evidence, evidence);
    persisted_document
        .validate(&committed_root)
        .expect("persisted proof document validates");
    let replay_error = fixture
        .nt_catalog_capability_proof
        .persist_completed_proof_from_evidence_guarded(
            &fixture.artifact_store,
            &writer,
            &evidence,
            &OperatorWorkBudgetGuard::unbounded(),
        )
        .await
        .expect_err("same proof artifact bytes must conflict with the occupied terminal key");
    assert!(
        format!("{replay_error:#}").contains("occupied"),
        "{replay_error:#}"
    );
    let mut changed_valid_evidence = evidence.clone();
    changed_valid_evidence.read_back.query_files_result_count += 1;
    let err = fixture
        .nt_catalog_capability_proof
        .persist_completed_proof_from_evidence_guarded(
            &fixture.artifact_store,
            &writer,
            &changed_valid_evidence,
            &OperatorWorkBudgetGuard::unbounded(),
        )
        .await
        .expect_err("changed proof artifact bytes must be rejected at the same URI");
    assert_error_chain_contains(&err, "occupied");

    let root = artifact_config().resolve().expect("valid artifact root");
    let mut proof = synthetic_capability_proof(
        &root,
        "s3-proof-run-123",
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        vec!["region".to_string()],
    );

    assert_eq!(proof.credential_source, NtCatalogCredentialSource::Ssm);
    proof
        .direct_s3_catalog_access_proven(&root)
        .expect("complete synthetic proof proves direct S3 catalog access");

    proof.synthetic_catalog_root_uri = root.nt_catalog_projection_root("canonical-projection");
    assert!(
        proof.direct_s3_catalog_access_proven(&root).is_err(),
        "capability proof must not point at the canonical NT catalog root"
    );

    let mut proof = synthetic_capability_proof(
        &root,
        "s3-proof-run-456",
        "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        vec!["region".to_string()],
    );
    proof.controls.invalid_credentials_write_failed = false;
    assert!(
        proof.direct_s3_catalog_access_proven(&root).is_err(),
        "credential negative control is required"
    );

    let mut proof = synthetic_capability_proof(
        &root,
        "s3-proof-run-789",
        "cccccccccccccccccccccccccccccccccccccccc",
        vec!["region".to_string()],
    );
    proof
        .synthetic_fixture_coverage
        .retain(|fixture| *fixture != MarketStructureFixture::BinaryOption);
    assert!(
        proof.direct_s3_catalog_access_proven(&root).is_err(),
        "binary-option synthetic fixture coverage is required"
    );
}

#[tokio::test]
async fn capability_proof_rejects_writer_bound_to_a_different_artifact_root_before_put() {
    let fixture = committed_capability_proof_fixture();
    let writer_root = fixture.artifact_store.resolve().expect("writer root");
    let mut requested_store = fixture.artifact_store.clone();
    requested_store.artifact_root = "s3://different-artifact-bucket/prod".to_string();
    let requested_root = requested_store.resolve().expect("requested root");
    let evidence =
        successful_capability_evidence(&requested_root, &fixture.nt_catalog_capability_proof);
    let store = VersionedCatalogStore::new();
    let writer = CreateOnlyArtifactWriter::new(&store, &writer_root);

    let error = fixture
        .nt_catalog_capability_proof
        .persist_completed_proof_from_evidence_guarded(
            &requested_store,
            &writer,
            &evidence,
            &OperatorWorkBudgetGuard::unbounded(),
        )
        .await
        .expect_err("mismatched writer root must fail before terminal publication");

    assert!(
        error
            .to_string()
            .contains("not bound to the supplied artifact root"),
        "{error:#}"
    );
    assert!(
        store.put_attempts().is_empty(),
        "writer/root mismatch reached the object store"
    );
}

#[tokio::test]
async fn capability_proof_serialization_budget_rejects_before_terminal_put() {
    let fixture = committed_capability_proof_fixture();
    let root = fixture.artifact_store.resolve().expect("artifact root");
    let evidence = successful_capability_evidence(&root, &fixture.nt_catalog_capability_proof);
    let store = VersionedCatalogStore::new();
    let writer = CreateOnlyArtifactWriter::new(&store, &root);
    let guard =
        OperatorWorkBudgetGuard::new(OperatorWorkBudget::Backfill(BackfillExecutionWorkBudget {
            max_decoded_bytes: 1,
            max_source_rows: 1,
            max_projected_row_groups: 1,
            max_wall_seconds: 60,
            require_object_selection_metadata: false,
        }))
        .expect("memory-capped guard");

    let error = fixture
        .nt_catalog_capability_proof
        .persist_completed_proof_from_evidence_guarded(
            &fixture.artifact_store,
            &writer,
            &evidence,
            &guard,
        )
        .await
        .expect_err("proof serialization must honor the work-budget cap");

    assert_error_chain_contains(&error, "max_decoded_bytes");
    assert!(store.put_attempts().is_empty());
}

#[tokio::test(start_paused = true)]
async fn committed_terminal_create_timeout_is_indeterminate_and_never_retries_put() {
    let fixture = committed_capability_proof_fixture();
    let root = fixture.artifact_store.resolve().expect("artifact root");
    let evidence = successful_capability_evidence(&root, &fixture.nt_catalog_capability_proof);
    let plan = fixture
        .nt_catalog_capability_proof
        .proof_plan(&fixture.artifact_store)
        .expect("proof plan");
    let proof_path = root
        .object_path_for_uri(&plan.proof_artifact_uri)
        .expect("proof path");
    let store = VersionedCatalogStore::new().with_pending_put(proof_path.clone());
    let writer = CreateOnlyArtifactWriter::new(&store, &root);

    let error = fixture
        .nt_catalog_capability_proof
        .persist_completed_proof_from_evidence_guarded(
            &fixture.artifact_store,
            &writer,
            &evidence,
            &OperatorWorkBudgetGuard::unbounded(),
        )
        .await
        .expect_err("pending terminal PUT must hit the configured hard tail bound");

    assert!(is_terminal_create_indeterminate(&error), "{error:#}");
    assert!(
        format!("{error:#}").contains("effective terminal timeout"),
        "{error:#}"
    );
    assert_eq!(
        store
            .put_attempts()
            .iter()
            .filter(|path| *path == &proof_path)
            .count(),
        1,
        "timed-out terminal creates must never retry automatically"
    );
    let committed_version = store
        .recorded_version(&proof_path)
        .expect("the pending acknowledgement follows a committed version");
    let committed_bytes = store
        .get_opts(
            &proof_path,
            GetOptions {
                version: Some(committed_version.0),
                ..GetOptions::default()
            },
        )
        .await
        .expect("the committed exact version remains readable")
        .bytes()
        .await
        .expect("committed proof bytes");
    assert!(!committed_bytes.is_empty());
}

#[tokio::test]
async fn capability_proof_expiry_after_preparation_writes_no_terminal_proof() {
    let fixture = committed_capability_proof_fixture();
    let root = fixture.artifact_store.resolve().expect("artifact root");
    let evidence = successful_capability_evidence(&root, &fixture.nt_catalog_capability_proof);
    let plan = fixture
        .nt_catalog_capability_proof
        .proof_plan(&fixture.artifact_store)
        .expect("proof plan");
    let proof_path = root
        .object_path_for_uri(&plan.proof_artifact_uri)
        .expect("proof path");
    let store = InMemory::new();
    let writer = CreateOnlyArtifactWriter::new(&store, &root);
    let guard = one_second_guard(Arc::new(ObservationExpiryClock {
        observations: AtomicUsize::new(0),
        // Construction and preparation precheck succeed; the unconditional
        // preparation postcheck expires before terminal authorization.
        expires_after_observation: 2,
    }));

    let error = fixture
        .nt_catalog_capability_proof
        .persist_completed_proof_from_evidence_guarded(
            &fixture.artifact_store,
            &writer,
            &evidence,
            &guard,
        )
        .await
        .expect_err("expiry after proof preparation must fence the terminal put");

    assert!(error.to_string().contains("max_wall_seconds"), "{error:#}");
    assert!(store.head(&proof_path).await.is_err());
}

#[tokio::test]
async fn capability_proof_commit_is_not_invalidated_by_a_later_clock_observation() {
    let fixture = committed_capability_proof_fixture();
    let root = fixture.artifact_store.resolve().expect("artifact root");
    let evidence = successful_capability_evidence(&root, &fixture.nt_catalog_capability_proof);
    let clock = Arc::new(ManualClock::default());
    let hook_clock = clock.clone();
    let store = VersionedCatalogStore::new().with_after_successful_put(Arc::new(
        move |_path, _sequence| hook_clock.set(Duration::from_secs(1)),
    ));
    let writer = CreateOnlyArtifactWriter::new(&store, &root);
    let guard = OperatorWorkBudgetGuard::with_clock(
        OperatorWorkBudget::Backfill(BackfillExecutionWorkBudget {
            max_decoded_bytes: u64::MAX,
            max_source_rows: 1,
            max_projected_row_groups: 10,
            max_wall_seconds: 1,
            require_object_selection_metadata: false,
        }),
        clock,
    )
    .expect("guard");

    fixture
        .nt_catalog_capability_proof
        .persist_completed_proof_from_evidence_guarded(
            &fixture.artifact_store,
            &writer,
            &evidence,
            &guard,
        )
        .await
        .expect("terminal permit authorizes the proof commit without a post-commit clock check");
}

#[test]
fn rejects_local_or_non_s3_canonical_artifact_roots() {
    let mut config = artifact_config();
    config.artifact_root = "/tmp/not-canonical".to_string();
    assert!(config.resolve().is_err());

    config.artifact_root = "file:///tmp/not-canonical".to_string();
    assert!(config.resolve().is_err());
}

#[test]
fn rejects_zero_terminal_commit_timeout() {
    let mut config = artifact_config();
    config.s3.terminal_commit_timeout_seconds = 0;

    let error = config
        .resolve()
        .expect_err("terminal commit timeout must be positive");
    assert!(
        error
            .to_string()
            .contains("terminal_commit_timeout_seconds must be positive"),
        "{error:#}"
    );
}

#[test]
fn rejects_invalid_s3_artifact_root_prefix_segments() {
    for artifact_root in [
        "s3://bolt-ra-artifacts/prod//bad",
        "s3://bolt-ra-artifacts/prod/./bad",
        "s3://bolt-ra-artifacts/prod/../bad",
    ] {
        let mut config = artifact_config();
        config.artifact_root = artifact_root.to_string();
        let err = config
            .resolve()
            .expect_err("invalid S3 prefix segment must be rejected");
        assert!(
            err.to_string().contains("artifact_root prefix"),
            "unexpected error for {artifact_root}: {err}"
        );
    }
}

#[test]
fn lifecycle_config_rejects_delete_expiration_and_keeps_hot_index_active() {
    let root = artifact_config().resolve().expect("valid artifact root");
    let policy = root.lifecycle_policy();

    assert_eq!(
        policy.state_after_quiet_window(ArtifactKind::Backtests, 3_599),
        ArtifactLifecycleState::Active
    );
    assert_eq!(
        policy.state_after_quiet_window(ArtifactKind::Backtests, 3_600),
        ArtifactLifecycleState::Inactive
    );
    assert_eq!(
        policy.hot_index_latest_pointer_storage_profile(),
        ArtifactStorageProfile::Active
    );
    assert_eq!(
        policy.hot_index_current_snapshot_storage_profile(),
        ArtifactStorageProfile::Active
    );

    let delete_enabled = artifact_config_toml().replace(
        "default_delete_expiration = \"disabled\"",
        "default_delete_expiration = \"enabled\"",
    );
    let config: ArtifactStoreConfig =
        toml::from_str(&delete_enabled).expect("delete-enabled config parses");
    let err = config
        .resolve()
        .expect_err("default delete/expiration must be rejected");
    assert!(err.to_string().contains("delete/expiration"), "{err}");

    let missing_deep_archive = artifact_config_toml().replace(
        "storage_profiles = [\"active\", \"archive\", \"deep_archive\"]",
        "storage_profiles = [\"active\", \"archive\"]",
    );
    let config: ArtifactStoreConfig =
        toml::from_str(&missing_deep_archive).expect("missing-profile config parses");
    let err = config
        .resolve()
        .expect_err("required lifecycle profile must be rejected");
    assert!(err.to_string().contains("deep_archive"), "{err}");
}

#[test]
fn dispatches_source_bindings_to_catalog_projection_roots_without_venue_paths() {
    let root = artifact_config().resolve().expect("valid artifact root");
    let dispatch = CatalogDispatchConfig {
        encoding: catalog_encoding(),
        bindings: vec![
            CatalogProjectionBinding {
                source_binding: "binary-official".to_string(),
                market_structure_fixture: MarketStructureFixture::BinaryOption,
                catalog_projection_id: "binary-projection-1".to_string(),
            },
            CatalogProjectionBinding {
                source_binding: "perps-official".to_string(),
                market_structure_fixture: MarketStructureFixture::PerpsSpot,
                catalog_projection_id: "perps-projection-1".to_string(),
            },
        ],
    };

    let binary = dispatch
        .catalog_root_for(
            "binary-official",
            MarketStructureFixture::BinaryOption,
            &root,
        )
        .expect("binary binding dispatches");
    let perps = dispatch
        .catalog_root_for("perps-official", MarketStructureFixture::PerpsSpot, &root)
        .expect("perps binding dispatches");

    assert_eq!(
        binary,
        "s3://bolt-ra-artifacts/prod/nt-catalog/v1/projection=binary-projection-1/"
    );
    assert_eq!(
        perps,
        "s3://bolt-ra-artifacts/prod/nt-catalog/v1/projection=perps-projection-1/"
    );
    assert!(!binary.contains("official"));
    assert!(!perps.contains("official"));
    assert!(
        dispatch
            .catalog_root_for(
                "missing-binding",
                MarketStructureFixture::BinaryOption,
                &root
            )
            .is_err()
    );
    let mismatch_err = dispatch
        .catalog_root_for("binary-official", MarketStructureFixture::PerpsSpot, &root)
        .expect_err("fixture mismatches must not dispatch to a durable catalog root");
    assert!(
        mismatch_err
            .to_string()
            .contains("market_structure_fixture mismatch"),
        "{mismatch_err}"
    );
}

#[tokio::test]
async fn create_only_probe_requires_duplicate_create_rejection() {
    let root = artifact_config().resolve().expect("valid artifact root");
    let store = InMemory::new();
    let writer = CreateOnlyArtifactWriter::new(&store, &root);

    let transcript = writer
        .probe_create_only_guarded(
            &root,
            "probe-run-123",
            &OperatorWorkBudgetGuard::unbounded(),
        )
        .await
        .expect("create-only probe");

    assert_eq!(
        transcript.probe_uri,
        "s3://bolt-ra-artifacts/prod/.writer-probe/probe=probe-run-123/sentinel"
    );
    assert_eq!(
        transcript.copy_source_uri,
        "s3://bolt-ra-artifacts/prod/.writer-probe/probe=probe-run-123/copy-source"
    );
    assert_eq!(
        transcript.copy_dest_uri,
        "s3://bolt-ra-artifacts/prod/.writer-probe/probe=probe-run-123/copy-dest"
    );
    assert!(transcript.first_create_succeeded);
    assert!(transcript.duplicate_create_rejected);
    assert!(transcript.first_copy_succeeded);
    assert!(transcript.duplicate_copy_rejected);
    let probe_path = root
        .object_path_for_uri(&transcript.probe_uri)
        .expect("probe uri under artifact root");
    let stored = store
        .get(&probe_path)
        .await
        .expect("created probe object")
        .bytes()
        .await
        .expect("probe object bytes");
    assert_eq!(stored.as_ref(), b"probe-run-123");
    let copied_path = root
        .object_path_for_uri(&transcript.copy_dest_uri)
        .expect("probe copy dest uri under artifact root");
    let copied = store
        .get(&copied_path)
        .await
        .expect("created probe copy object")
        .bytes()
        .await
        .expect("probe copy object bytes");
    assert_eq!(copied.as_ref(), b"probe-run-123");
}

#[tokio::test]
async fn create_only_probe_expiry_after_setup_writes_no_probe_object() {
    let root = artifact_config().resolve().expect("valid artifact root");
    let store = InMemory::new();
    let writer = CreateOnlyArtifactWriter::new(&store, &root);
    let guard = one_second_guard(Arc::new(ObservationExpiryClock {
        observations: AtomicUsize::new(0),
        expires_after_observation: 2,
    }));
    let probe_uri = root.create_only_probe_uri("probe-run-expiry");
    let probe_path = root
        .object_path_for_uri(&probe_uri)
        .expect("probe path under root");

    let error = writer
        .probe_create_only_guarded(&root, "probe-run-expiry", &guard)
        .await
        .expect_err("expiry before the first irreversible probe write must fail closed");

    assert!(error.to_string().contains("max_wall_seconds"), "{error:#}");
    assert!(store.head(&probe_path).await.is_err());
}

#[tokio::test]
async fn create_only_probe_rejects_existing_same_payload_sentinels() {
    let root = artifact_config().resolve().expect("valid artifact root");
    let store = InMemory::new();
    let writer = CreateOnlyArtifactWriter::new(&store, &root);

    let first = writer
        .probe_create_only_guarded(
            &root,
            "probe-run-123",
            &OperatorWorkBudgetGuard::unbounded(),
        )
        .await
        .expect("first create-only probe");
    let error = writer
        .probe_create_only_guarded(
            &root,
            "probe-run-123",
            &OperatorWorkBudgetGuard::unbounded(),
        )
        .await
        .expect_err("same-payload probe replay must fail strict creation");

    assert!(first.first_create_succeeded);
    assert!(first.duplicate_create_rejected);
    assert!(first.first_copy_succeeded);
    assert!(first.duplicate_copy_rejected);
    assert!(format!("{error:#}").contains("create-only put"));
}

fn catalog_dispatch(projection_id: &str) -> CatalogDispatchConfig {
    CatalogDispatchConfig {
        encoding: catalog_encoding(),
        bindings: vec![CatalogProjectionBinding {
            source_binding: "binary-official".to_string(),
            market_structure_fixture: MarketStructureFixture::BinaryOption,
            catalog_projection_id: projection_id.to_string(),
        }],
    }
}

fn catalog_encoding() -> CatalogEncodingConfig {
    CatalogEncodingConfig::new(5000, 5000, CatalogCompression::Snappy)
        .expect("positive test catalog encoding")
}

fn write_catalog_file(root: &Path, relative_path: &str, payload: &[u8]) {
    let path = root.join(relative_path);
    fs::create_dir_all(path.parent().expect("catalog object parent"))
        .expect("create catalog object parent");
    fs::write(path, payload).expect("write catalog object");
}

fn create_private_hydration_root(parent: &Path, name: &str) -> PathBuf {
    let root = parent.join(name);
    fs::create_dir(&root).expect("create private hydration root");
    #[cfg(unix)]
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700))
        .expect("set private hydration root mode");
    root
}

#[tokio::test]
async fn catalog_publication_receipt_crosslinks_shared_manifest_and_versions_and_is_last() {
    let root = artifact_config().resolve().expect("valid artifact root");
    let dispatch = catalog_dispatch("projection-run-123");
    let temp = tempfile::TempDir::new().expect("temp dir");
    let trade_path = "data/trade_tick/instrument=BTC-USD.BINARY/part-000.parquet";
    let metadata_path = "metadata/instrument=BTC-USD.BINARY/part-000.parquet";
    write_catalog_file(temp.path(), trade_path, b"trade-ticks");
    write_catalog_file(temp.path(), metadata_path, b"instrument-metadata");
    let physical_manifest = catalog_physical_manifest(&[
        (trade_path, b"trade-ticks"),
        (metadata_path, b"instrument-metadata"),
    ]);
    let store = VersionedCatalogStore::new();

    let persisted = persist_catalog_projection_for_source_binding_guarded(
        &store,
        &root,
        &root.emulate_bucket_versioning_enabled_for_contract_test(),
        &dispatch,
        "binary-official",
        MarketStructureFixture::BinaryOption,
        temp.path(),
        &physical_manifest,
        &OperatorWorkBudgetGuard::unbounded(),
    )
    .await
    .expect("catalog publication");

    assert_eq!(
        persisted.catalog_root_uri,
        "s3://bolt-ra-artifacts/prod/nt-catalog/v1/projection=projection-run-123/"
    );
    assert_eq!(
        persisted.receipt_uri,
        "s3://bolt-ra-artifacts/prod/nt-catalog/v1/projection=projection-run-123/catalog-projection-manifest.json"
    );
    assert_eq!(persisted.objects.len(), physical_manifest.objects.len());
    assert_eq!(
        persisted
            .objects
            .iter()
            .map(|object| object.relative_path.as_str())
            .collect::<Vec<_>>(),
        physical_manifest
            .objects
            .iter()
            .map(|object| object.relative_path.as_str())
            .collect::<Vec<_>>()
    );
    assert!(
        persisted
            .objects
            .iter()
            .all(|object| !object.version_id.is_empty()
                && object.create_only_write == CreateOnlyWriteDisposition::Created)
    );
    let expected_manifest_sha256 = physical_manifest
        .manifest_sha256_guarded(
            &OperatorWorkBudgetGuard::unbounded(),
            OperatorWorkBudgetStage::Publish,
        )
        .expect("shared physical manifest hash");
    assert_eq!(persisted.physical_manifest_sha256, expected_manifest_sha256);
    assert!(!persisted.receipt_version_id.is_empty());
    assert_eq!(
        persisted.receipt_create_only_write,
        CreateOnlyWriteDisposition::Created
    );

    let receipt_path = root
        .object_path_for_uri(&persisted.receipt_uri)
        .expect("receipt path");
    let receipt_bytes = store
        .get(&receipt_path)
        .await
        .expect("receipt object")
        .bytes()
        .await
        .expect("receipt bytes");
    assert_eq!(persisted.receipt_sha256, sha256_hex(&receipt_bytes));
    let receipt: serde_json::Value =
        serde_json::from_slice(&receipt_bytes).expect("publication receipt JSON");
    assert_eq!(
        receipt["schema_version"].as_str(),
        Some("catalog-projection-publication-receipt-v2")
    );
    assert_eq!(
        receipt["physical_manifest_sha256"].as_str(),
        Some(persisted.physical_manifest_sha256.as_str())
    );
    assert_eq!(
        receipt["physical_manifest"],
        serde_json::to_value(&physical_manifest).expect("physical manifest value")
    );
    assert_eq!(
        receipt["binding"]["source_binding"].as_str(),
        Some("binary-official")
    );
    let receipt_objects = receipt["objects"].as_array().expect("receipt objects");
    assert_eq!(receipt_objects.len(), persisted.objects.len());
    for (receipt_object, persisted_object) in receipt_objects.iter().zip(&persisted.objects) {
        assert_eq!(
            receipt_object["relative_path"].as_str(),
            Some(persisted_object.relative_path.as_str())
        );
        assert_eq!(
            receipt_object["uri"].as_str(),
            Some(persisted_object.uri.as_str())
        );
        assert_eq!(
            receipt_object["version_id"].as_str(),
            Some(persisted_object.version_id.as_str())
        );
        assert_eq!(
            receipt_object["e_tag"].as_str(),
            Some(persisted_object.e_tag.as_str())
        );
        assert_eq!(
            receipt_object["sha256"].as_str(),
            Some(persisted_object.sha256.as_str())
        );
        assert_eq!(
            receipt_object["byte_len"].as_u64(),
            Some(persisted_object.byte_len)
        );
        // A conditional create can succeed after an S3 delete marker. The
        // committed exact version/hash/length tuple, not create-only status,
        // is therefore the immutable read authority.
        assert!(receipt_object.get("create_only_write").is_none());
    }
    assert_eq!(store.put_attempts().last(), Some(&receipt_path));

    let restarted_receipt = CatalogProjectionPublicationReceipt::parse_and_validate_guarded(
        &receipt_bytes,
        &persisted.receipt_sha256,
        &OperatorWorkBudgetGuard::unbounded(),
        OperatorWorkBudgetStage::Publish,
    )
    .expect("receipt must be loadable and authoritative after process restart");
    assert_eq!(restarted_receipt.physical_manifest, physical_manifest);
    assert_eq!(
        restarted_receipt.objects[0].version_id,
        persisted.objects[0].version_id
    );

    let authority_uri = format!(
        "{}{}",
        persisted.catalog_root_uri, CATALOG_RUN_VIEW_AUTHORITY_FILE
    );
    let authority_path = root
        .object_path_for_uri(&authority_uri)
        .expect("authority remote path");
    assert!(store.head(&authority_path).await.is_err());
    assert!(!temp.path().join(CATALOG_RUN_VIEW_AUTHORITY_FILE).exists());
}

#[tokio::test]
async fn catalog_publication_rejects_an_existing_catalog_even_when_bytes_match() {
    let root = artifact_config().resolve().expect("valid artifact root");
    let dispatch = catalog_dispatch("projection-idempotent");
    let temp = tempfile::TempDir::new().expect("temp dir");
    write_catalog_file(temp.path(), "part-000.parquet", b"same-catalog");
    let physical_manifest = catalog_physical_manifest(&[("part-000.parquet", b"same-catalog")]);
    let store = VersionedCatalogStore::new();

    let first = persist_catalog_projection_for_source_binding_guarded(
        &store,
        &root,
        &root.emulate_bucket_versioning_enabled_for_contract_test(),
        &dispatch,
        "binary-official",
        MarketStructureFixture::BinaryOption,
        temp.path(),
        &physical_manifest,
        &OperatorWorkBudgetGuard::unbounded(),
    )
    .await
    .expect("first publication");
    let error = persist_catalog_projection_for_source_binding_guarded(
        &store,
        &root,
        &root.emulate_bucket_versioning_enabled_for_contract_test(),
        &dispatch,
        "binary-official",
        MarketStructureFixture::BinaryOption,
        temp.path(),
        &physical_manifest,
        &OperatorWorkBudgetGuard::unbounded(),
    )
    .await
    .expect_err("fresh publication must reject an occupied catalog object");

    assert!(format!("{error:#}").contains("occupied"), "{error:#}");
    assert_eq!(
        store.recorded_version(
            &root
                .object_path_for_uri(&first.objects[0].uri)
                .expect("catalog object path")
        ),
        Some((
            first.objects[0].version_id.clone(),
            Some(first.objects[0].e_tag.clone())
        )),
        "the rejected replay must not replace the original immutable object"
    );
    assert!(
        store.exact_version_get_attempts().is_empty(),
        "fresh publication must not read an occupied object into success"
    );
}

#[tokio::test]
async fn catalog_publication_stops_on_a_lost_receipt_ack_without_recovery() {
    let root = artifact_config().resolve().expect("valid artifact root");
    let dispatch = catalog_dispatch("projection-lost-receipt-ack");
    let temp = tempfile::TempDir::new().expect("temp dir");
    write_catalog_file(temp.path(), "part-000.parquet", b"same-catalog");
    let physical_manifest = catalog_physical_manifest(&[("part-000.parquet", b"same-catalog")]);
    let receipt_path = root
        .object_path_for_uri(
            &root.catalog_projection_manifest_object_uri("projection-lost-receipt-ack"),
        )
        .expect("receipt path");
    let store = VersionedCatalogStore::new().with_lost_put_ack(receipt_path.clone());

    let error = persist_catalog_projection_for_source_binding_guarded(
        &store,
        &root,
        &root.emulate_bucket_versioning_enabled_for_contract_test(),
        &dispatch,
        "binary-official",
        MarketStructureFixture::BinaryOption,
        temp.path(),
        &physical_manifest,
        &OperatorWorkBudgetGuard::unbounded(),
    )
    .await
    .expect_err("lost receipt acknowledgement must stop as indeterminate");

    assert!(is_terminal_create_indeterminate(&error), "{error:#}");
    assert!(
        store.recorded_version(&receipt_path).is_some(),
        "the synthetic store committed before dropping the acknowledgement"
    );
    assert_eq!(
        store
            .put_attempts()
            .iter()
            .filter(|path| *path == &receipt_path)
            .count(),
        1,
        "an indeterminate terminal create must never be retried"
    );
    assert!(
        store
            .exact_version_get_attempts()
            .iter()
            .all(|(path, _, _)| path != &receipt_path),
        "fresh publication must not reconcile an indeterminate acknowledgement"
    );
}

#[tokio::test]
async fn catalog_publication_rejects_different_bytes_at_terminal_receipt_key() {
    let root = artifact_config().resolve().expect("valid artifact root");
    let dispatch = catalog_dispatch("projection-conflicting-receipt");
    let temp = tempfile::TempDir::new().expect("temp dir");
    write_catalog_file(temp.path(), "part-000.parquet", b"catalog");
    let physical_manifest = catalog_physical_manifest(&[("part-000.parquet", b"catalog")]);
    let receipt_path = root
        .object_path_for_uri(
            &root.catalog_projection_manifest_object_uri("projection-conflicting-receipt"),
        )
        .expect("receipt path");
    let store = VersionedCatalogStore::new();
    store
        .put_opts(
            &receipt_path,
            b"occupied-by-different-bytes".to_vec().into(),
            PutMode::Overwrite.into(),
        )
        .await
        .expect("seed conflicting versioned receipt");

    let error = persist_catalog_projection_for_source_binding_guarded(
        &store,
        &root,
        &root.emulate_bucket_versioning_enabled_for_contract_test(),
        &dispatch,
        "binary-official",
        MarketStructureFixture::BinaryOption,
        temp.path(),
        &physical_manifest,
        &OperatorWorkBudgetGuard::unbounded(),
    )
    .await
    .expect_err("different terminal bytes must fail as a hard conflict");

    assert!(!is_terminal_create_indeterminate(&error), "{error:#}");
    assert!(
        format!("{error:#}").contains("occupied terminal key"),
        "{error:#}"
    );
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn exact_version_receipt_hydration_builds_only_the_shared_physical_manifest() {
    let root = artifact_config().resolve().expect("valid artifact root");
    let dispatch = catalog_dispatch("projection-hydration");
    let source = tempfile::TempDir::new().expect("source temp dir");
    let first_path = "data/trades/instrument=BTC-USD/part-000.parquet";
    let second_path = "data/trades/instrument=ETH-USD/part-000.parquet";
    write_catalog_file(source.path(), first_path, b"btc-trades");
    write_catalog_file(source.path(), second_path, b"eth-trades");
    let physical_manifest =
        catalog_physical_manifest(&[(first_path, b"btc-trades"), (second_path, b"eth-trades")]);
    let store = VersionedCatalogStore::new();
    let persisted = persist_catalog_projection_for_source_binding_guarded(
        &store,
        &root,
        &root.emulate_bucket_versioning_enabled_for_contract_test(),
        &dispatch,
        "binary-official",
        MarketStructureFixture::BinaryOption,
        source.path(),
        &physical_manifest,
        &OperatorWorkBudgetGuard::unbounded(),
    )
    .await
    .expect("publish hydration source");
    let hydration_parent = tempfile::TempDir::new().expect("hydration parent");
    let hydration_root =
        create_private_hydration_root(hydration_parent.path(), "catalog-hydration");
    let guard = OperatorWorkBudgetGuard::unbounded();
    let heartbeat = tokio::spawn(async {
        tokio::task::yield_now().await;
    });

    let hydrated = hydrate_catalog_projection_from_receipt_guarded(
        &store,
        &root,
        &persisted.receipt_locator(),
        &physical_manifest,
        &hydration_root,
        &guard,
    )
    .await
    .expect("exact-version hydration");

    assert!(
        heartbeat.is_finished(),
        "hydration disk writes must yield the async runtime"
    );
    heartbeat.await.expect("hydration heartbeat");
    assert_eq!(hydrated.local_catalog_root(), hydration_root);
    assert_eq!(hydrated.object_count, physical_manifest.objects.len());
    assert_eq!(hydrated.receipt_sha256, persisted.receipt_sha256);
    assert_eq!(
        fs::metadata(&hydration_root)
            .expect("hydration root metadata")
            .permissions()
            .mode()
            & 0o777,
        0o700,
        "hydration root remains private and writable for stable retry"
    );
    assert_eq!(
        fs::metadata(hydration_root.join("data"))
            .expect("hydration data directory metadata")
            .permissions()
            .mode()
            & 0o777,
        0o500,
        "hydrated nested directories are sealed read/execute-only"
    );
    assert_eq!(
        fs::metadata(hydration_root.join(first_path))
            .expect("hydrated first object metadata")
            .permissions()
            .mode()
            & 0o777,
        0o400,
        "hydrated files are sealed owner-read-only"
    );
    assert_eq!(
        fs::read(hydration_root.join(first_path)).expect("hydrated first object"),
        b"btc-trades"
    );
    assert_eq!(
        fs::read(hydration_root.join(second_path)).expect("hydrated second object"),
        b"eth-trades"
    );
    assert!(
        !hydration_root
            .join(CATALOG_RUN_VIEW_AUTHORITY_FILE)
            .exists()
    );
    assert!(
        !hydration_root
            .join("catalog-projection-manifest.json")
            .exists(),
        "remote receipt must not enter the hydrated NT root"
    );
    hydrated
        .revalidate_for_runner_seal_guarded(&physical_manifest, &guard)
        .expect("retained root and exact set revalidate; runner owns content hashes");
}

#[tokio::test]
async fn hydration_requires_receipt_etag_and_exact_response_match() {
    let root = artifact_config().resolve().expect("valid artifact root");
    let dispatch = catalog_dispatch("projection-receipt-etag");
    let source = tempfile::TempDir::new().expect("source temp dir");
    write_catalog_file(source.path(), "part-000.parquet", b"catalog");
    let physical_manifest = catalog_physical_manifest(&[("part-000.parquet", b"catalog")]);
    let store = VersionedCatalogStore::new();
    let persisted = persist_catalog_projection_for_source_binding_guarded(
        &store,
        &root,
        &root.emulate_bucket_versioning_enabled_for_contract_test(),
        &dispatch,
        "binary-official",
        MarketStructureFixture::BinaryOption,
        source.path(),
        &physical_manifest,
        &OperatorWorkBudgetGuard::unbounded(),
    )
    .await
    .expect("publish hydration source");
    let receipt_path = root
        .object_path_for_uri(&persisted.receipt_uri)
        .expect("receipt path");

    store.omit_exact_get_e_tag_once(receipt_path);
    let hydration_parent = tempfile::TempDir::new().expect("hydration parent");
    let hydration_root =
        create_private_hydration_root(hydration_parent.path(), "catalog-hydration");
    let error = hydrate_catalog_projection_from_receipt_guarded(
        &store,
        &root,
        &persisted.receipt_locator(),
        &physical_manifest,
        &hydration_root,
        &OperatorWorkBudgetGuard::unbounded(),
    )
    .await
    .expect_err("missing exact receipt response ETag must fail closed");
    assert!(format!("{error:#}").contains("ETag"), "{error:#}");

    let mut blank_locator = persisted.receipt_locator();
    blank_locator.receipt_e_tag = "   ".to_string();
    let exact_gets_before = store.exact_version_get_attempts().len();
    let blank_hydration_parent = tempfile::TempDir::new().expect("blank hydration parent");
    let blank_hydration_root =
        create_private_hydration_root(blank_hydration_parent.path(), "catalog-hydration");
    let error = hydrate_catalog_projection_from_receipt_guarded(
        &store,
        &root,
        &blank_locator,
        &physical_manifest,
        &blank_hydration_root,
        &OperatorWorkBudgetGuard::unbounded(),
    )
    .await
    .expect_err("blank receipt locator ETag must fail before exact GET");
    assert!(format!("{error:#}").contains("ETag"), "{error:#}");
    assert_eq!(store.exact_version_get_attempts().len(), exact_gets_before);

    let mut zero_length_locator = persisted.receipt_locator();
    zero_length_locator.receipt_byte_len = 0;
    let error = hydrate_catalog_projection_from_receipt_guarded(
        &store,
        &root,
        &zero_length_locator,
        &physical_manifest,
        &blank_hydration_root,
        &OperatorWorkBudgetGuard::unbounded(),
    )
    .await
    .expect_err("zero receipt locator length must fail before exact GET");
    assert!(format!("{error:#}").contains("byte length"), "{error:#}");
    assert_eq!(store.exact_version_get_attempts().len(), exact_gets_before);

    let mut wrong_length_locator = persisted.receipt_locator();
    wrong_length_locator.receipt_byte_len += 1;
    let error = hydrate_catalog_projection_from_receipt_guarded(
        &store,
        &root,
        &wrong_length_locator,
        &physical_manifest,
        &blank_hydration_root,
        &OperatorWorkBudgetGuard::unbounded(),
    )
    .await
    .expect_err("receipt response length must match the pinned locator before collection");
    assert!(
        format!("{error:#}").contains("instead of exact expected"),
        "{error:#}"
    );
}

#[tokio::test]
async fn hydration_rejects_wrong_receipt_version_before_touching_private_root() {
    let root = artifact_config().resolve().expect("valid artifact root");
    let dispatch = catalog_dispatch("projection-wrong-receipt-version");
    let source = tempfile::TempDir::new().expect("source temp dir");
    write_catalog_file(source.path(), "part-000.parquet", b"catalog");
    let physical_manifest = catalog_physical_manifest(&[("part-000.parquet", b"catalog")]);
    let store = VersionedCatalogStore::new();
    let persisted = persist_catalog_projection_for_source_binding_guarded(
        &store,
        &root,
        &root.emulate_bucket_versioning_enabled_for_contract_test(),
        &dispatch,
        "binary-official",
        MarketStructureFixture::BinaryOption,
        source.path(),
        &physical_manifest,
        &OperatorWorkBudgetGuard::unbounded(),
    )
    .await
    .expect("publish hydration source");
    let mut locator = persisted.receipt_locator();
    locator.receipt_version_id = "wrong-version".to_string();
    let hydration_parent = tempfile::TempDir::new().expect("hydration parent");
    let hydration_root =
        create_private_hydration_root(hydration_parent.path(), "catalog-hydration");

    let error = hydrate_catalog_projection_from_receipt_guarded(
        &store,
        &root,
        &locator,
        &physical_manifest,
        &hydration_root,
        &OperatorWorkBudgetGuard::unbounded(),
    )
    .await
    .expect_err("wrong receipt version must fail closed");

    assert!(format!("{error:#}").contains("exact version"), "{error:#}");
    assert!(
        fs::read_dir(&hydration_root)
            .expect("hydration root")
            .next()
            .is_none()
    );
}

#[tokio::test]
async fn hydration_rejects_missing_returned_object_version_without_authority() {
    let root = artifact_config().resolve().expect("valid artifact root");
    let dispatch = catalog_dispatch("projection-missing-hydration-version");
    let source = tempfile::TempDir::new().expect("source temp dir");
    write_catalog_file(source.path(), "part-000.parquet", b"catalog");
    let physical_manifest = catalog_physical_manifest(&[("part-000.parquet", b"catalog")]);
    let store = VersionedCatalogStore::new();
    let persisted = persist_catalog_projection_for_source_binding_guarded(
        &store,
        &root,
        &root.emulate_bucket_versioning_enabled_for_contract_test(),
        &dispatch,
        "binary-official",
        MarketStructureFixture::BinaryOption,
        source.path(),
        &physical_manifest,
        &OperatorWorkBudgetGuard::unbounded(),
    )
    .await
    .expect("publish hydration source");
    let object_path = root
        .object_path_for_uri(&persisted.objects[0].uri)
        .expect("published object path");
    store.omit_version(object_path);
    let hydration_parent = tempfile::TempDir::new().expect("hydration parent");
    let hydration_root =
        create_private_hydration_root(hydration_parent.path(), "catalog-hydration");

    let error = hydrate_catalog_projection_from_receipt_guarded(
        &store,
        &root,
        &persisted.receipt_locator(),
        &physical_manifest,
        &hydration_root,
        &OperatorWorkBudgetGuard::unbounded(),
    )
    .await
    .expect_err("missing returned object version must fail closed");

    assert!(format!("{error:#}").contains("version"), "{error:#}");
    assert!(
        !hydration_root
            .join(CATALOG_RUN_VIEW_AUTHORITY_FILE)
            .exists()
    );
}

#[cfg(unix)]
#[tokio::test]
async fn hydration_fd_relative_traversal_rejects_intermediate_symlink_swap() {
    let root = artifact_config().resolve().expect("valid artifact root");
    let dispatch = catalog_dispatch("projection-hydration-symlink-swap");
    let source = tempfile::TempDir::new().expect("source temp dir");
    let relative_path = "data/trades/part-000.parquet";
    write_catalog_file(source.path(), relative_path, b"catalog");
    let physical_manifest = catalog_physical_manifest(&[(relative_path, b"catalog")]);
    let hydration_parent = tempfile::TempDir::new().expect("hydration parent");
    let hydration_root =
        create_private_hydration_root(hydration_parent.path(), "catalog-hydration");
    let outside = tempfile::TempDir::new().expect("outside hydration target");
    let symlink_path = hydration_root.join("data");
    let outside_path = outside.path().to_path_buf();
    let store =
        VersionedCatalogStore::new().with_after_successful_get(Arc::new(move |_path, sequence| {
            if sequence == 2 {
                std::os::unix::fs::symlink(&outside_path, &symlink_path)
                    .expect("swap intermediate hydration directory to symlink");
            }
        }));
    let persisted = persist_catalog_projection_for_source_binding_guarded(
        &store,
        &root,
        &root.emulate_bucket_versioning_enabled_for_contract_test(),
        &dispatch,
        "binary-official",
        MarketStructureFixture::BinaryOption,
        source.path(),
        &physical_manifest,
        &OperatorWorkBudgetGuard::unbounded(),
    )
    .await
    .expect("publish hydration source");

    let error = hydrate_catalog_projection_from_receipt_guarded(
        &store,
        &root,
        &persisted.receipt_locator(),
        &physical_manifest,
        &hydration_root,
        &OperatorWorkBudgetGuard::unbounded(),
    )
    .await
    .expect_err("fd-relative traversal must reject symlink substitution");

    assert!(
        format!("{error:#}").contains("without following symlinks")
            || format!("{error:#}").contains("openat"),
        "{error:#}"
    );
    assert!(!outside.path().join("trades/part-000.parquet").exists());
    assert!(!outside.path().join("part-000.parquet").exists());
}

#[tokio::test]
async fn hydration_root_lease_rejects_path_replacement_before_runner_seal() {
    let root = artifact_config().resolve().expect("valid artifact root");
    let dispatch = catalog_dispatch("projection-root-lease");
    let source = tempfile::TempDir::new().expect("source temp dir");
    write_catalog_file(source.path(), "part-000.parquet", b"catalog");
    let physical_manifest = catalog_physical_manifest(&[("part-000.parquet", b"catalog")]);
    let store = VersionedCatalogStore::new();
    let persisted = persist_catalog_projection_for_source_binding_guarded(
        &store,
        &root,
        &root.emulate_bucket_versioning_enabled_for_contract_test(),
        &dispatch,
        "binary-official",
        MarketStructureFixture::BinaryOption,
        source.path(),
        &physical_manifest,
        &OperatorWorkBudgetGuard::unbounded(),
    )
    .await
    .expect("publish hydration source");
    let hydration_parent = tempfile::TempDir::new().expect("hydration parent");
    let hydration_root =
        create_private_hydration_root(hydration_parent.path(), "catalog-hydration");
    let hydrated = hydrate_catalog_projection_from_receipt_guarded(
        &store,
        &root,
        &persisted.receipt_locator(),
        &physical_manifest,
        &hydration_root,
        &OperatorWorkBudgetGuard::unbounded(),
    )
    .await
    .expect("hydrate catalog");
    let displaced = hydration_parent.path().join("displaced-catalog");
    fs::rename(&hydration_root, &displaced).expect("displace hydrated root");
    let replacement = create_private_hydration_root(hydration_parent.path(), "catalog-hydration");
    write_catalog_file(&replacement, "part-000.parquet", b"catalog");

    let error = hydrated
        .revalidate_for_runner_seal_guarded(
            &physical_manifest,
            &OperatorWorkBudgetGuard::unbounded(),
        )
        .expect_err("retained lease must reject root path replacement");

    assert!(format!("{error:#}").contains("identity"), "{error:#}");
}

#[test]
fn receipt_parse_rejects_noncanonical_json_even_when_hash_matches_bytes() {
    let physical_manifest = catalog_physical_manifest(&[("part-000.parquet", b"catalog")]);
    let physical_manifest_sha256 = physical_manifest
        .manifest_sha256_guarded(
            &OperatorWorkBudgetGuard::unbounded(),
            OperatorWorkBudgetStage::Publish,
        )
        .expect("physical manifest hash");
    let receipt = serde_json::json!({
        "schema_version": "catalog-projection-publication-receipt-v2",
        "catalog_root_uri": "s3://bolt-ra-artifacts/prod/nt-catalog/v1/projection=canonical/",
        "physical_manifest_sha256": physical_manifest_sha256,
        "physical_manifest": physical_manifest,
        "binding": {
            "source_binding": "binary-official",
            "market_structure_fixture": "binary-option",
            "catalog_projection_id": "canonical"
        },
        "objects": [{
            "relative_path": "part-000.parquet",
            "uri": "s3://bolt-ra-artifacts/prod/nt-catalog/v1/projection=canonical/part-000.parquet",
            "sha256": sha256_hex(b"catalog"),
            "byte_len": 7,
            "version_id": "version-1",
            "e_tag": "etag-1"
        }]
    });
    let noncanonical = serde_json::to_vec_pretty(&receipt).expect("noncanonical receipt bytes");
    let noncanonical_hash = sha256_hex(&noncanonical);

    let error = CatalogProjectionPublicationReceipt::parse_and_validate_guarded(
        &noncanonical,
        &noncanonical_hash,
        &OperatorWorkBudgetGuard::unbounded(),
        OperatorWorkBudgetStage::ObjectVerification,
    )
    .expect_err("pretty JSON must not pass canonical receipt validation");

    assert!(format!("{error:#}").contains("not canonical"), "{error:#}");
}

#[test]
fn catalog_receipt_requires_nonempty_object_etag() {
    let physical_manifest = catalog_physical_manifest(&[("part-000.parquet", b"catalog")]);
    let physical_manifest_sha256 = physical_manifest
        .manifest_sha256_guarded(
            &OperatorWorkBudgetGuard::unbounded(),
            OperatorWorkBudgetStage::Publish,
        )
        .expect("physical manifest hash");
    let mut receipt = CatalogProjectionPublicationReceipt {
        schema_version: "catalog-projection-publication-receipt-v2".to_string(),
        catalog_root_uri:
            "s3://bolt-ra-artifacts/prod/nt-catalog/v1/projection=etag-required/".to_string(),
        physical_manifest_sha256,
        physical_manifest,
        binding: CatalogProjectionBinding {
            source_binding: "binary-official".to_string(),
            market_structure_fixture: MarketStructureFixture::BinaryOption,
            catalog_projection_id: "etag-required".to_string(),
        },
        objects: vec![CatalogProjectionPublicationObject {
            relative_path: "part-000.parquet".to_string(),
            uri: "s3://bolt-ra-artifacts/prod/nt-catalog/v1/projection=etag-required/part-000.parquet"
                .to_string(),
            sha256: sha256_hex(b"catalog"),
            byte_len: 7,
            version_id: "version-1".to_string(),
            e_tag: "etag-1".to_string(),
        }],
    };

    let mut missing = serde_json::to_value(&receipt).expect("serialize receipt");
    missing["objects"][0]
        .as_object_mut()
        .expect("receipt object")
        .remove("e_tag");
    let error = serde_json::from_value::<CatalogProjectionPublicationReceipt>(missing)
        .expect_err("missing catalog object ETag must fail deserialization");
    assert!(error.to_string().contains("e_tag"), "{error:#}");

    receipt.objects[0].e_tag = "   ".to_string();
    let error = receipt
        .canonical_bytes_guarded(
            &OperatorWorkBudgetGuard::unbounded(),
            OperatorWorkBudgetStage::Publish,
        )
        .expect_err("blank catalog object ETag must fail validation");
    assert!(error.to_string().contains("ETag"), "{error:#}");
}

#[tokio::test]
async fn catalog_manifest_hash_mismatch_fails_before_any_put() {
    let root = artifact_config().resolve().expect("valid artifact root");
    let dispatch = catalog_dispatch("projection-manifest-mismatch");
    let temp = tempfile::TempDir::new().expect("temp dir");
    write_catalog_file(temp.path(), "part-000.parquet", b"actual");
    let physical_manifest = catalog_physical_manifest(&[("part-000.parquet", b"absent")]);
    let store = VersionedCatalogStore::new();

    let error = persist_catalog_projection_for_source_binding_guarded(
        &store,
        &root,
        &root.emulate_bucket_versioning_enabled_for_contract_test(),
        &dispatch,
        "binary-official",
        MarketStructureFixture::BinaryOption,
        temp.path(),
        &physical_manifest,
        &OperatorWorkBudgetGuard::unbounded(),
    )
    .await
    .expect_err("producer manifest mismatch must fail before publication");

    assert!(format!("{error:#}").contains("SHA-256"), "{error:#}");
    assert!(store.put_attempts().is_empty());
}

#[tokio::test]
async fn catalog_projection_accepts_file_at_exact_final_object_cap() {
    const FINAL_OBJECT_CAP: u64 = 4_096;
    let mut config = artifact_config();
    config.max_final_object_bytes = FINAL_OBJECT_CAP;
    let root = config.resolve().expect("valid exact-cap artifact root");
    let dispatch = catalog_dispatch("projection-exact-cap");
    let temp = tempfile::TempDir::new().expect("temp dir");
    let payload = vec![0x5a; FINAL_OBJECT_CAP as usize];
    write_catalog_file(temp.path(), "part-000.parquet", &payload);
    let physical_manifest = catalog_physical_manifest(&[("part-000.parquet", payload.as_slice())]);
    let store = VersionedCatalogStore::new();

    let persisted = persist_catalog_projection_for_source_binding_guarded(
        &store,
        &root,
        &root.emulate_bucket_versioning_enabled_for_contract_test(),
        &dispatch,
        "binary-official",
        MarketStructureFixture::BinaryOption,
        temp.path(),
        &physical_manifest,
        &OperatorWorkBudgetGuard::unbounded(),
    )
    .await
    .expect("exact-cap object publication");

    assert_eq!(persisted.objects[0].byte_len, FINAL_OBJECT_CAP);
}

#[tokio::test]
async fn catalog_projection_work_budget_memory_cap_rejects_before_single_put() {
    let root = artifact_config().resolve().expect("valid artifact root");
    let dispatch = catalog_dispatch("projection-memory-cap");
    let temp = tempfile::TempDir::new().expect("temp dir");
    let payload = vec![0x5a; 4_096];
    write_catalog_file(temp.path(), "part-000.parquet", &payload);
    let physical_manifest = catalog_physical_manifest(&[("part-000.parquet", payload.as_slice())]);
    let work_budget =
        OperatorWorkBudgetGuard::new(OperatorWorkBudget::Backfill(BackfillExecutionWorkBudget {
            max_decoded_bytes: 1_024,
            max_source_rows: 1,
            max_projected_row_groups: 8,
            max_wall_seconds: 60,
            require_object_selection_metadata: false,
        }))
        .expect("memory-capped work budget");
    let store = VersionedCatalogStore::new();

    let error = persist_catalog_projection_for_source_binding_guarded(
        &store,
        &root,
        &root.emulate_bucket_versioning_enabled_for_contract_test(),
        &dispatch,
        "binary-official",
        MarketStructureFixture::BinaryOption,
        temp.path(),
        &physical_manifest,
        &work_budget,
    )
    .await
    .expect_err("work-budget memory cap must reject a protocol-valid object");

    assert!(
        format!("{error:#}").contains("max_decoded_bytes"),
        "{error:#}"
    );
    assert!(store.put_attempts().is_empty());
}

#[tokio::test]
async fn catalog_projection_rejects_cap_plus_one_before_any_object_put() {
    const FINAL_OBJECT_CAP: u64 = 8;
    let mut config = artifact_config();
    config.max_final_object_bytes = FINAL_OBJECT_CAP;
    let root = config.resolve().expect("valid cap-plus-one artifact root");
    let dispatch = catalog_dispatch("projection-cap-plus-one");
    let temp = tempfile::TempDir::new().expect("temp dir");
    let at_cap = vec![0x5a; FINAL_OBJECT_CAP as usize];
    let over_cap = vec![0x5a; FINAL_OBJECT_CAP as usize + 1];
    write_catalog_file(temp.path(), "at-cap.parquet", &at_cap);
    write_catalog_file(temp.path(), "cap-plus-one.parquet", &over_cap);
    let physical_manifest = catalog_physical_manifest(&[
        ("at-cap.parquet", at_cap.as_slice()),
        ("cap-plus-one.parquet", over_cap.as_slice()),
    ]);
    let store = VersionedCatalogStore::new();

    let error = persist_catalog_projection_for_source_binding_guarded(
        &store,
        &root,
        &root.emulate_bucket_versioning_enabled_for_contract_test(),
        &dispatch,
        "binary-official",
        MarketStructureFixture::BinaryOption,
        temp.path(),
        &physical_manifest,
        &OperatorWorkBudgetGuard::unbounded(),
    )
    .await
    .expect_err("cap-plus-one object must fail before publication");

    assert!(
        format!("{error:#}").contains("cap-plus-one.parquet"),
        "{error:#}"
    );
    assert!(
        format!("{error:#}").contains("max_final_object_bytes"),
        "{error:#}"
    );
    assert!(store.put_attempts().is_empty());
}

#[tokio::test]
async fn catalog_publication_receipt_payload_obeys_final_object_cap() {
    let mut config = artifact_config();
    config.max_final_object_bytes = 1;
    let root = config.resolve().expect("valid one-byte artifact root");
    let dispatch = catalog_dispatch("projection-receipt-cap");
    let temp = tempfile::TempDir::new().expect("temp dir");
    write_catalog_file(temp.path(), "one-byte.parquet", &[0x5a]);
    let physical_manifest = catalog_physical_manifest(&[("one-byte.parquet", &[0x5a])]);
    let store = VersionedCatalogStore::new();

    let error = persist_catalog_projection_for_source_binding_guarded(
        &store,
        &root,
        &root.emulate_bucket_versioning_enabled_for_contract_test(),
        &dispatch,
        "binary-official",
        MarketStructureFixture::BinaryOption,
        temp.path(),
        &physical_manifest,
        &OperatorWorkBudgetGuard::unbounded(),
    )
    .await
    .expect_err("oversized receipt must fail closed");

    assert!(
        format!("{error:#}").contains("publication receipt payload"),
        "{error:#}"
    );
    let receipt_path = root
        .object_path_for_uri(&root.catalog_projection_manifest_object_uri("projection-receipt-cap"))
        .expect("receipt path");
    assert!(store.head(&receipt_path).await.is_err());
}

#[tokio::test]
async fn catalog_projection_expiry_before_publication_writes_no_object() {
    let root = artifact_config().resolve().expect("valid artifact root");
    let dispatch = catalog_dispatch("projection-expiry");
    let temp = tempfile::TempDir::new().expect("temp dir");
    write_catalog_file(temp.path(), "part-000.parquet", b"catalog");
    let physical_manifest = catalog_physical_manifest(&[("part-000.parquet", b"catalog")]);
    let store = VersionedCatalogStore::new();
    let guard = one_second_guard(Arc::new(ObservationExpiryClock {
        observations: AtomicUsize::new(0),
        expires_after_observation: 1,
    }));

    let error = persist_catalog_projection_for_source_binding_guarded(
        &store,
        &root,
        &root.emulate_bucket_versioning_enabled_for_contract_test(),
        &dispatch,
        "binary-official",
        MarketStructureFixture::BinaryOption,
        temp.path(),
        &physical_manifest,
        &guard,
    )
    .await
    .expect_err("expired work budget must fence publication");

    assert!(error.to_string().contains("max_wall_seconds"), "{error:#}");
    assert!(store.put_attempts().is_empty());
}

#[cfg(unix)]
#[tokio::test]
async fn rejects_catalog_projection_symlink_without_following() {
    let root = artifact_config().resolve().expect("valid artifact root");
    let dispatch = catalog_dispatch("projection-symlink");
    let temp = tempfile::TempDir::new().expect("temp dir");
    let outside = tempfile::TempDir::new().expect("outside dir");
    write_catalog_file(outside.path(), "part-000.parquet", b"outside-root");
    std::os::unix::fs::symlink(outside.path(), temp.path().join("linked-catalog"))
        .expect("catalog symlink");
    let physical_manifest =
        catalog_physical_manifest(&[("linked-catalog/part-000.parquet", b"outside-root")]);
    let store = VersionedCatalogStore::new();

    let error = persist_catalog_projection_for_source_binding_guarded(
        &store,
        &root,
        &root.emulate_bucket_versioning_enabled_for_contract_test(),
        &dispatch,
        "binary-official",
        MarketStructureFixture::BinaryOption,
        temp.path(),
        &physical_manifest,
        &OperatorWorkBudgetGuard::unbounded(),
    )
    .await
    .expect_err("catalog symlink must fail closed");

    assert!(format!("{error:#}").contains("non-regular"), "{error:#}");
    assert!(store.put_attempts().is_empty());
}

#[tokio::test]
async fn changed_authorized_catalog_bytes_conflict_with_existing_immutable_object() {
    let root = artifact_config().resolve().expect("valid artifact root");
    let dispatch = catalog_dispatch("projection-conflict");
    let temp = tempfile::TempDir::new().expect("temp dir");
    write_catalog_file(temp.path(), "part-000.parquet", b"first");
    let first_manifest = catalog_physical_manifest(&[("part-000.parquet", b"first")]);
    let store = VersionedCatalogStore::new();
    persist_catalog_projection_for_source_binding_guarded(
        &store,
        &root,
        &root.emulate_bucket_versioning_enabled_for_contract_test(),
        &dispatch,
        "binary-official",
        MarketStructureFixture::BinaryOption,
        temp.path(),
        &first_manifest,
        &OperatorWorkBudgetGuard::unbounded(),
    )
    .await
    .expect("first immutable publication");
    write_catalog_file(temp.path(), "part-000.parquet", b"other");
    let changed_manifest = catalog_physical_manifest(&[("part-000.parquet", b"other")]);

    let error = persist_catalog_projection_for_source_binding_guarded(
        &store,
        &root,
        &root.emulate_bucket_versioning_enabled_for_contract_test(),
        &dispatch,
        "binary-official",
        MarketStructureFixture::BinaryOption,
        temp.path(),
        &changed_manifest,
        &OperatorWorkBudgetGuard::unbounded(),
    )
    .await
    .expect_err("changed immutable object must conflict");

    assert!(format!("{error:#}").contains("occupied key"), "{error:#}");
}

#[tokio::test]
async fn catalog_publication_rejects_late_stray_before_receipt() {
    let root = artifact_config().resolve().expect("valid artifact root");
    let dispatch = catalog_dispatch("projection-late-stray");
    let temp = tempfile::TempDir::new().expect("temp dir");
    write_catalog_file(temp.path(), "a.parquet", b"first");
    write_catalog_file(temp.path(), "b.parquet", b"second");
    let physical_manifest =
        catalog_physical_manifest(&[("a.parquet", b"first"), ("b.parquet", b"second")]);
    let stray_path = temp.path().join("late-stray.parquet");
    let store =
        VersionedCatalogStore::new().with_after_successful_put(Arc::new(move |_path, sequence| {
            if sequence == 1 {
                fs::write(&stray_path, b"stray").expect("plant late stray");
            }
        }));

    let error = persist_catalog_projection_for_source_binding_guarded(
        &store,
        &root,
        &root.emulate_bucket_versioning_enabled_for_contract_test(),
        &dispatch,
        "binary-official",
        MarketStructureFixture::BinaryOption,
        temp.path(),
        &physical_manifest,
        &OperatorWorkBudgetGuard::unbounded(),
    )
    .await
    .expect_err("late stray must prevent receipt");

    assert!(
        format!("{error:#}").contains("unexpected object"),
        "{error:#}"
    );
    let receipt_path = root
        .object_path_for_uri(&root.catalog_projection_manifest_object_uri("projection-late-stray"))
        .expect("receipt path");
    assert!(store.head(&receipt_path).await.is_err());
}

#[tokio::test]
async fn catalog_publication_rejects_missing_object_before_receipt() {
    let root = artifact_config().resolve().expect("valid artifact root");
    let dispatch = catalog_dispatch("projection-late-missing");
    let temp = tempfile::TempDir::new().expect("temp dir");
    write_catalog_file(temp.path(), "a.parquet", b"first");
    write_catalog_file(temp.path(), "b.parquet", b"second");
    let physical_manifest =
        catalog_physical_manifest(&[("a.parquet", b"first"), ("b.parquet", b"second")]);
    let missing_path = temp.path().join("b.parquet");
    let store =
        VersionedCatalogStore::new().with_after_successful_put(Arc::new(move |_path, sequence| {
            if sequence == 1 {
                fs::remove_file(&missing_path).expect("remove pending catalog object");
            }
        }));

    let error = persist_catalog_projection_for_source_binding_guarded(
        &store,
        &root,
        &root.emulate_bucket_versioning_enabled_for_contract_test(),
        &dispatch,
        "binary-official",
        MarketStructureFixture::BinaryOption,
        temp.path(),
        &physical_manifest,
        &OperatorWorkBudgetGuard::unbounded(),
    )
    .await
    .expect_err("missing object must prevent receipt");

    assert!(format!("{error:#}").contains("b.parquet"), "{error:#}");
    let receipt_path = root
        .object_path_for_uri(
            &root.catalog_projection_manifest_object_uri("projection-late-missing"),
        )
        .expect("receipt path");
    assert!(store.head(&receipt_path).await.is_err());
}

#[tokio::test]
async fn catalog_publication_rejects_same_length_mutation_after_put() {
    let root = artifact_config().resolve().expect("valid artifact root");
    let dispatch = catalog_dispatch("projection-late-mutation");
    let temp = tempfile::TempDir::new().expect("temp dir");
    write_catalog_file(temp.path(), "part-000.parquet", b"first");
    let physical_manifest = catalog_physical_manifest(&[("part-000.parquet", b"first")]);
    let mutation_path = temp.path().join("part-000.parquet");
    let store =
        VersionedCatalogStore::new().with_after_successful_put(Arc::new(move |_path, sequence| {
            if sequence == 1 {
                fs::write(&mutation_path, b"other").expect("same-length mutation");
            }
        }));

    let error = persist_catalog_projection_for_source_binding_guarded(
        &store,
        &root,
        &root.emulate_bucket_versioning_enabled_for_contract_test(),
        &dispatch,
        "binary-official",
        MarketStructureFixture::BinaryOption,
        temp.path(),
        &physical_manifest,
        &OperatorWorkBudgetGuard::unbounded(),
    )
    .await
    .expect_err("same-length mutation must prevent receipt");

    assert!(
        format!("{error:#}").contains("identity") || format!("{error:#}").contains("changed"),
        "{error:#}"
    );
}

#[tokio::test]
async fn catalog_publication_requires_object_version_id() {
    let root = artifact_config().resolve().expect("valid artifact root");
    let dispatch = catalog_dispatch("projection-object-version");
    let temp = tempfile::TempDir::new().expect("temp dir");
    write_catalog_file(temp.path(), "part-000.parquet", b"catalog");
    let physical_manifest = catalog_physical_manifest(&[("part-000.parquet", b"catalog")]);
    let object_uri = format!(
        "{}part-000.parquet",
        root.nt_catalog_projection_root("projection-object-version")
    );
    let object_path = root
        .object_path_for_uri(&object_uri)
        .expect("catalog object path");
    let store = VersionedCatalogStore::new().with_omitted_version(object_path);

    let error = persist_catalog_projection_for_source_binding_guarded(
        &store,
        &root,
        &root.emulate_bucket_versioning_enabled_for_contract_test(),
        &dispatch,
        "binary-official",
        MarketStructureFixture::BinaryOption,
        temp.path(),
        &physical_manifest,
        &OperatorWorkBudgetGuard::unbounded(),
    )
    .await
    .expect_err("missing object version must fail closed");

    assert!(format!("{error:#}").contains("version ID"), "{error:#}");
    assert_eq!(store.put_attempts().len(), 1);
}

#[tokio::test]
async fn catalog_publication_rejects_a_missing_receipt_version_without_recovery() {
    let root = artifact_config().resolve().expect("valid artifact root");
    let dispatch = catalog_dispatch("projection-receipt-version");
    let temp = tempfile::TempDir::new().expect("temp dir");
    write_catalog_file(temp.path(), "part-000.parquet", b"catalog");
    let physical_manifest = catalog_physical_manifest(&[("part-000.parquet", b"catalog")]);
    let receipt_path = root
        .object_path_for_uri(
            &root.catalog_projection_manifest_object_uri("projection-receipt-version"),
        )
        .expect("receipt path");
    let store = VersionedCatalogStore::new().with_omitted_version(receipt_path.clone());

    let error = persist_catalog_projection_for_source_binding_guarded(
        &store,
        &root,
        &root.emulate_bucket_versioning_enabled_for_contract_test(),
        &dispatch,
        "binary-official",
        MarketStructureFixture::BinaryOption,
        temp.path(),
        &physical_manifest,
        &OperatorWorkBudgetGuard::unbounded(),
    )
    .await
    .expect_err("missing acknowledgement version must stop as indeterminate");

    assert!(is_terminal_create_indeterminate(&error), "{error:#}");
    assert_eq!(
        store
            .put_attempts()
            .iter()
            .filter(|path| *path == &receipt_path)
            .count(),
        1
    );
    assert_eq!(
        store
            .exact_version_get_attempts()
            .iter()
            .filter(|(path, _, _)| path == &receipt_path)
            .count(),
        0,
        "strict publication must not recover an unusable acknowledgement"
    );
}

#[tokio::test]
async fn catalog_publication_rejects_an_unusable_put_etag_without_recovery() {
    let root = artifact_config().resolve().expect("valid artifact root");

    for (projection_id, blank_put_etag) in [
        ("projection-missing-put-etag", false),
        ("projection-blank-put-etag", true),
    ] {
        let dispatch = catalog_dispatch(projection_id);
        let temp = tempfile::TempDir::new().expect("temp dir");
        write_catalog_file(temp.path(), "part-000.parquet", b"catalog");
        let physical_manifest = catalog_physical_manifest(&[("part-000.parquet", b"catalog")]);
        let receipt_path = root
            .object_path_for_uri(&root.catalog_projection_manifest_object_uri(projection_id))
            .expect("receipt path");
        let store = if blank_put_etag {
            VersionedCatalogStore::new().with_blank_put_e_tag(receipt_path.clone())
        } else {
            VersionedCatalogStore::new().with_omitted_put_e_tag(receipt_path.clone())
        };

        let error = persist_catalog_projection_for_source_binding_guarded(
            &store,
            &root,
            &root.emulate_bucket_versioning_enabled_for_contract_test(),
            &dispatch,
            "binary-official",
            MarketStructureFixture::BinaryOption,
            temp.path(),
            &physical_manifest,
            &OperatorWorkBudgetGuard::unbounded(),
        )
        .await
        .expect_err("unusable PUT ETag must stop as indeterminate");

        assert!(is_terminal_create_indeterminate(&error), "{error:#}");
        assert!(format!("{error:#}").contains("ETag"), "{error:#}");
        assert!(
            store
                .exact_version_get_attempts()
                .iter()
                .all(|(path, _, _)| path != &receipt_path),
            "strict publication must not recover an unusable acknowledgement"
        );
    }
}

#[tokio::test]
async fn catalog_publication_rejects_a_null_receipt_version_without_recovery() {
    let root = artifact_config().resolve().expect("valid artifact root");
    let dispatch = catalog_dispatch("projection-null-receipt-version");
    let temp = tempfile::TempDir::new().expect("temp dir");
    write_catalog_file(temp.path(), "part-000.parquet", b"catalog");
    let physical_manifest = catalog_physical_manifest(&[("part-000.parquet", b"catalog")]);
    let receipt_path = root
        .object_path_for_uri(
            &root.catalog_projection_manifest_object_uri("projection-null-receipt-version"),
        )
        .expect("receipt path");
    let store = VersionedCatalogStore::new().with_null_version(receipt_path.clone());

    let error = persist_catalog_projection_for_source_binding_guarded(
        &store,
        &root,
        &root.emulate_bucket_versioning_enabled_for_contract_test(),
        &dispatch,
        "binary-official",
        MarketStructureFixture::BinaryOption,
        temp.path(),
        &physical_manifest,
        &OperatorWorkBudgetGuard::unbounded(),
    )
    .await
    .expect_err("null acknowledgement version must stop as indeterminate");

    assert!(is_terminal_create_indeterminate(&error), "{error:#}");
    assert_eq!(
        store
            .exact_version_get_attempts()
            .iter()
            .filter(|(path, _, _)| path == &receipt_path)
            .count(),
        0,
        "strict publication must not recover an unusable acknowledgement"
    );
}

#[tokio::test]
async fn empty_receipt_version_is_typed_indeterminate_without_recovery() {
    let root = artifact_config().resolve().expect("valid artifact root");
    let dispatch = catalog_dispatch("projection-empty-receipt-version");
    let temp = tempfile::TempDir::new().expect("temp dir");
    write_catalog_file(temp.path(), "part-000.parquet", b"catalog");
    let physical_manifest = catalog_physical_manifest(&[("part-000.parquet", b"catalog")]);
    let receipt_path = root
        .object_path_for_uri(
            &root.catalog_projection_manifest_object_uri("projection-empty-receipt-version"),
        )
        .expect("receipt path");
    let store = VersionedCatalogStore::new().with_empty_version(receipt_path.clone());

    let error = persist_catalog_projection_for_source_binding_guarded(
        &store,
        &root,
        &root.emulate_bucket_versioning_enabled_for_contract_test(),
        &dispatch,
        "binary-official",
        MarketStructureFixture::BinaryOption,
        temp.path(),
        &physical_manifest,
        &OperatorWorkBudgetGuard::unbounded(),
    )
    .await
    .expect_err("unprovable empty acknowledgement version must be indeterminate");

    assert!(is_terminal_create_indeterminate(&error), "{error:#}");
    assert_eq!(
        store
            .put_attempts()
            .iter()
            .filter(|path| *path == &receipt_path)
            .count(),
        1
    );
    assert_eq!(
        store
            .exact_version_get_attempts()
            .iter()
            .filter(|(path, _, _)| path == &receipt_path)
            .count(),
        0,
        "strict publication must not recover an unusable acknowledgement"
    );
}

#[tokio::test]
async fn broad_catalog_publication_keeps_manifest_order_without_retained_file_vector() {
    const OBJECT_COUNT: usize = 64;
    let root = artifact_config().resolve().expect("valid artifact root");
    let dispatch = catalog_dispatch("projection-broad");
    let temp = tempfile::TempDir::new().expect("temp dir");
    let mut objects = Vec::new();
    for index in 0..OBJECT_COUNT {
        let relative_path = format!("data/trades/instrument={index:04}/part-000.parquet");
        let payload = format!("catalog-{index:04}").into_bytes();
        write_catalog_file(temp.path(), &relative_path, &payload);
        objects.push(CatalogProjectionManifestObject {
            relative_path,
            byte_len: u64::try_from(payload.len()).expect("payload length"),
            sha256: sha256_hex(&payload),
        });
    }
    let physical_manifest = CatalogProjectionManifestDocument {
        schema_version: CATALOG_PROJECTION_MANIFEST_SCHEMA_VERSION.to_string(),
        objects,
    };
    let store = VersionedCatalogStore::new();

    let persisted = persist_catalog_projection_for_source_binding_guarded(
        &store,
        &root,
        &root.emulate_bucket_versioning_enabled_for_contract_test(),
        &dispatch,
        "binary-official",
        MarketStructureFixture::BinaryOption,
        temp.path(),
        &physical_manifest,
        &OperatorWorkBudgetGuard::unbounded(),
    )
    .await
    .expect("broad catalog publication");

    assert_eq!(persisted.objects.len(), OBJECT_COUNT);
    assert_eq!(store.put_attempts().len(), OBJECT_COUNT + 1);
    let source = include_str!("../src/artifact_store.rs");
    assert!(!source.contains("struct PreparedCatalogProjectionFile"));
    assert!(!source.contains("struct CatalogProjectionManifestDocument<'"));
    assert!(!source.contains("Vec<fs::File>"));
    assert!(!source.contains("preflight_catalog_projection_contents_guarded"));
}

#[tokio::test]
async fn rejects_catalog_dispatch_fixture_mismatch_before_any_put() {
    let root = artifact_config().resolve().expect("valid artifact root");
    let dispatch = catalog_dispatch("projection-fixture-mismatch");
    let temp = tempfile::TempDir::new().expect("temp dir");
    write_catalog_file(temp.path(), "part-000.parquet", b"fixture-mismatch");
    let physical_manifest = catalog_physical_manifest(&[("part-000.parquet", b"fixture-mismatch")]);
    let store = VersionedCatalogStore::new();

    let error = persist_catalog_projection_for_source_binding_guarded(
        &store,
        &root,
        &root.emulate_bucket_versioning_enabled_for_contract_test(),
        &dispatch,
        "binary-official",
        MarketStructureFixture::PerpsSpot,
        temp.path(),
        &physical_manifest,
        &OperatorWorkBudgetGuard::unbounded(),
    )
    .await
    .expect_err("market-structure fixture mismatch must reject publication");

    assert!(
        error
            .to_string()
            .contains("market_structure_fixture mismatch"),
        "{error:#}"
    );
    assert!(store.put_attempts().is_empty());
}

#[test]
fn rejects_manifest_fixture_mismatch() {
    let gz = gzip(SAMPLE_CSV);
    let mut spec = committed_run_spec_for(&gz);
    spec.artifact_store = None;
    spec.manifest.market_structure_fixture = MarketStructureFixture::BinaryOption;
    let output_dir = tempfile::TempDir::new().expect("temp dir");

    let err = match run_from_run_spec(&spec, &gz, output_dir.path()) {
        Ok(_) => panic!("manifest fixture must match accepted source proof fixture_type"),
        Err(err) => err,
    };

    assert!(
        err.to_string().contains("accepted.fixture_type"),
        "FixtureMismatch not reported: {err}"
    );
}

#[tokio::test]
async fn operator_artifact_store_path_rejects_artifact_store_region_mismatch_before_local_work() {
    let gz = gzip(SAMPLE_CSV);
    let mut spec = committed_run_spec_for(&gz);
    spec.manifest
        .artifact_store
        .rust_storage_options
        .insert("region".to_string(), "us-west-2".to_string());
    let output_dir = tempfile::TempDir::new().expect("temp dir");
    let store = InMemory::new();
    let artifact_root = spec
        .required_artifact_store()
        .expect("artifact-store config")
        .resolve()
        .expect("artifact root");
    let versioning = artifact_root.emulate_bucket_versioning_enabled_for_contract_test();

    let work_budget = OperatorWorkBudgetGuard::unbounded();
    let err = match run_from_run_spec_with_artifact_store_guarded(
        &spec,
        gz,
        output_dir.path(),
        &store,
        &versioning,
        &work_budget,
    )
    .await
    {
        Ok(_) => panic!("artifact-store region mismatch must fail"),
        Err(err) => err,
    };

    assert!(
        err.to_string().contains("artifact-store region mismatch"),
        "{err}"
    );
    assert!(
        !output_dir.path().join(CONVERSION_MANIFEST_FILE).exists(),
        "artifact-store config mismatch must fail before local conversion work"
    );
}

#[tokio::test]
async fn operator_artifact_store_path_rejects_legacy_storage_options_before_local_work() {
    let gz = gzip(SAMPLE_CSV);
    let mut spec = committed_run_spec_for(&gz);
    let configured_region = spec
        .required_artifact_store()
        .expect("artifact-store config")
        .s3
        .region
        .clone();
    spec.manifest
        .artifact_store
        .storage_options
        .insert("region".to_string(), configured_region);
    let output_dir = tempfile::TempDir::new().expect("temp dir");
    let store = InMemory::new();
    let artifact_root = spec
        .required_artifact_store()
        .expect("artifact-store config")
        .resolve()
        .expect("artifact root");
    let versioning = artifact_root.emulate_bucket_versioning_enabled_for_contract_test();
    let work_budget = OperatorWorkBudgetGuard::unbounded();

    let error = run_from_run_spec_with_artifact_store_guarded(
        &spec,
        gz,
        output_dir.path(),
        &store,
        &versioning,
        &work_budget,
    )
    .await
    .err()
    .expect("legacy storage_options must not remain an alternate durable config authority");

    assert!(
        error.to_string().contains("storage_options is retired"),
        "{error:#}"
    );
    assert!(
        !output_dir.path().join(CONVERSION_MANIFEST_FILE).exists(),
        "legacy storage_options must fail before local conversion work"
    );
}

#[tokio::test]
async fn operator_artifact_store_path_rejects_artifact_store_ssm_region_mismatch_before_local_work()
{
    let gz = gzip(SAMPLE_CSV);
    let mut spec = committed_run_spec_for(&gz);
    spec.manifest.artifact_store.ssm_parameters = Some(ManifestArtifactStoreSsmParameters {
        region: "us-west-2".to_string(),
        access_key_id: "/bolt-v2/test/access-key-id".to_string(),
        secret_access_key: "/bolt-v2/test/secret-access-key".to_string(),
        session_token: None,
    });
    let output_dir = tempfile::TempDir::new().expect("temp dir");
    let store = InMemory::new();
    let artifact_root = spec
        .required_artifact_store()
        .expect("artifact-store config")
        .resolve()
        .expect("artifact root");
    let versioning = artifact_root.emulate_bucket_versioning_enabled_for_contract_test();

    let work_budget = OperatorWorkBudgetGuard::unbounded();
    let err = match run_from_run_spec_with_artifact_store_guarded(
        &spec,
        gz,
        output_dir.path(),
        &store,
        &versioning,
        &work_budget,
    )
    .await
    {
        Ok(_) => panic!("artifact-store SSM region mismatch must fail"),
        Err(err) => err,
    };

    assert!(
        err.to_string()
            .contains("artifact-store SSM region mismatch"),
        "{err}"
    );
    assert!(
        !output_dir.path().join(CONVERSION_MANIFEST_FILE).exists(),
        "artifact-store SSM config mismatch must fail before local conversion work"
    );
}

#[tokio::test]
async fn operator_artifact_store_path_persists_catalog_and_rewrites_contract_uri() {
    const LEGACY_V3_DURABLE_COMPLETION_MANIFEST_FILE: &str = "durable-completion-manifest.v3.json";

    assert_eq!(
        DURABLE_COMPLETION_MANIFEST_FILE, "durable-completion-manifest.v4.json",
        "the ETag-bound v4 schema must own an explicitly versioned immutable key"
    );
    let gz = gzip(SAMPLE_CSV);
    let spec = committed_run_spec_for(&gz);
    let output_dir = tempfile::TempDir::new().expect("temp dir");
    let artifact_store = spec
        .required_artifact_store()
        .expect("artifact-store config");
    let catalog_dispatch = spec
        .required_catalog_dispatch()
        .expect("catalog dispatch config");
    let artifact_root = artifact_store.resolve().expect("artifact root resolves");
    let completion_uri = format!(
        "{}/{}",
        spec.manifest.output_prefix.trim_end_matches('/'),
        DURABLE_COMPLETION_MANIFEST_FILE
    );
    let completion_path = artifact_root
        .object_path_for_uri(&completion_uri)
        .expect("durable completion path");
    let legacy_completion_uri = format!(
        "{}/{}",
        spec.manifest.output_prefix.trim_end_matches('/'),
        LEGACY_V3_DURABLE_COMPLETION_MANIFEST_FILE
    );
    let legacy_completion_path = artifact_root
        .object_path_for_uri(&legacy_completion_uri)
        .expect("legacy durable completion path");
    assert_ne!(
        completion_path, legacy_completion_path,
        "ETag-bound v4 terminals must not reuse the immutable v3 terminal key"
    );
    let candidate_path = output_dir
        .path()
        .join(OPERATOR_DURABLE_OUTPUT_CANDIDATE_SEAL_FILE);
    let terminal_put_observed_candidate = Arc::new(AtomicBool::new(false));
    let terminal_put_observed_candidate_for_hook = Arc::clone(&terminal_put_observed_candidate);
    let completion_path_for_hook = completion_path.clone();
    let candidate_path_for_hook = candidate_path.clone();
    let store = VersionedCatalogStore::new().with_after_successful_put(Arc::new(move |path, _| {
        if path == &completion_path_for_hook {
            assert!(
                candidate_path_for_hook.is_file(),
                "local output candidate seal must precede the remote terminal PUT"
            );
            terminal_put_observed_candidate_for_hook.store(true, Ordering::SeqCst);
        }
    }));
    let legacy_completion_bytes: &[u8] = b"preexisting immutable v3 durable terminal";
    store
        .put_opts(
            &legacy_completion_path,
            legacy_completion_bytes.to_vec().into(),
            PutMode::Overwrite.into(),
        )
        .await
        .expect("seed immutable legacy v3 durable terminal");
    let expected_catalog_root = catalog_dispatch
        .catalog_root_for(
            &spec.source_proof.source_binding,
            spec.manifest.market_structure_fixture,
            &artifact_root,
        )
        .expect("source binding dispatches");
    let versioning = artifact_root.emulate_bucket_versioning_enabled_for_contract_test();
    let source_bindings = VerifiedSourceBindingRegistry::from_run_spec(&spec)
        .expect("snapshot source bindings for durable discovery");
    assert_current_durable_completion_absent_with_artifact_store_guarded(
        &spec,
        &store,
        &versioning,
        &source_bindings,
        &OperatorWorkBudgetGuard::unbounded(),
    )
    .await
    .expect("legacy v3 key must not interfere with the v4 absence check");

    let work_budget = OperatorWorkBudgetGuard::unbounded();
    let outcome = run_from_run_spec_with_artifact_store_guarded(
        &spec,
        gz.clone(),
        output_dir.path(),
        &store,
        &versioning,
        &work_budget,
    )
    .await
    .expect("operator artifact-store run");
    let completion_locator = outcome.receipt().completion.clone();
    let original_execution_attestation = outcome.receipt().execution_attestation.clone();
    assert_eq!(
        original_execution_attestation.provenance,
        DurableExecutionProvenance::ExecutedProcessIsolated
    );
    assert!(
        terminal_put_observed_candidate.load(Ordering::SeqCst),
        "remote terminal publication must observe the pre-terminal local candidate seal"
    );
    assert_eq!(completion_locator.object.uri, completion_uri);
    assert_eq!(
        store
            .get(&legacy_completion_path)
            .await
            .expect("legacy v3 durable terminal remains readable")
            .bytes()
            .await
            .expect("read legacy v3 durable terminal bytes")
            .as_ref(),
        legacy_completion_bytes,
        "v4 publication must not mutate the immutable v3 terminal"
    );
    assert_eq!(
        store
            .put_attempts()
            .iter()
            .filter(|path| *path == &legacy_completion_path)
            .count(),
        1,
        "only the test seed may write the legacy v3 terminal key"
    );
    assert_eq!(
        store
            .exact_version_get_attempts()
            .iter()
            .filter(|(path, _, _)| path == &completion_path)
            .count(),
        0,
        "fresh durable completion publication must accept only its direct create acknowledgement"
    );
    let artifacts = executed_durable_artifacts(outcome);

    assert_eq!(
        artifacts.canonical_catalog_uri.as_deref(),
        Some(expected_catalog_root.as_str())
    );
    assert!(
        artifacts.catalog_root.is_dir(),
        "artifact-store path retains the identity-owned projection for deterministic retry and audit"
    );
    let persisted_projection = artifacts
        .persisted_catalog_projection
        .as_ref()
        .expect("durable run persists the exact catalog projection");
    assert_eq!(artifacts.output.catalog_run_view_authority.roots.len(), 1);
    assert_eq!(
        artifacts.output.catalog_run_view_authority.roots[0].physical_manifest_sha256,
        persisted_projection.physical_manifest_sha256,
        "retained local projection and immutable S3 receipt must bind the same physical manifest"
    );
    assert_eq!(
        artifacts.output.contract.artifact_uris.nt_catalog_uri,
        expected_catalog_root
    );
    let transient_catalog_uri = format!(
        "{}/{}",
        spec.manifest.output_prefix.trim_end_matches('/'),
        CATALOG_DIR
    );
    for claim_limit in &artifacts.output.contract.claim_limits {
        assert!(
            !claim_limit.contains(&transient_catalog_uri),
            "durable contract claim limit must not reference transient catalog URI: {claim_limit}"
        );
    }
    assert!(
        artifacts.output.contract.claim_limits.iter().any(|limit| {
            limit.contains("NT pass_through surface catalog.catalog_path")
                && limit.contains(&expected_catalog_root)
        }),
        "durable contract claim limits must reference the persisted catalog root"
    );
    assert!(
        !artifacts.persisted_catalog_objects.is_empty(),
        "operator must persist projected catalog objects through artifact-store dispatch"
    );
    let persisted_projection = artifacts
        .persisted_catalog_projection
        .as_ref()
        .expect("operator must expose persisted catalog projection proof");
    assert_eq!(
        persisted_projection.receipt_create_only_write,
        CreateOnlyWriteDisposition::Created
    );
    assert_eq!(
        persisted_projection.binding.source_binding,
        spec.source_proof.source_binding
    );
    assert!(!persisted_projection.physical_manifest_sha256.is_empty());
    assert!(
        persisted_projection
            .objects
            .iter()
            .all(|object| !object.version_id.is_empty())
    );
    assert_eq!(
        artifacts.output.contract.catalog_hash,
        artifacts.output.conversion_catalog_metadata.catalog_hash,
        "durable result contract must keep catalog_hash coherent with catalog metadata"
    );
    let CatalogConsumption::HydratedPublication { receipt } = &artifacts
        .output
        .conversion_catalog_metadata
        .catalog_consumption
    else {
        panic!("artifact-store path must record exact-version hydrated publication evidence")
    };
    assert_eq!(receipt.catalog_root_uri, expected_catalog_root);
    assert_eq!(receipt.receipt_uri, persisted_projection.receipt_uri);
    assert_eq!(receipt.receipt_sha256, persisted_projection.receipt_sha256);
    assert_eq!(
        receipt.receipt_version_id,
        persisted_projection.receipt_version_id
    );
    assert_eq!(receipt.receipt_e_tag, persisted_projection.receipt_e_tag);
    assert_eq!(
        receipt.physical_manifest_sha256,
        persisted_projection.physical_manifest_sha256
    );
    assert_eq!(
        artifacts.output.contract.catalog_metadata_hash,
        artifacts
            .output
            .conversion_catalog_metadata
            .content_hash()
            .expect("catalog metadata hash"),
        "durable result contract must bind the rewritten catalog metadata"
    );
    let persisted_metadata: ConversionCatalogMetadata = serde_json::from_str(
        &fs::read_to_string(&artifacts.catalog_metadata_path).expect("catalog metadata json"),
    )
    .expect("catalog metadata parses");
    assert_eq!(
        persisted_metadata, artifacts.output.conversion_catalog_metadata,
        "catalog-metadata.json must be rewritten with durable execution access"
    );
    assert_eq!(
        artifacts
            .output
            .contract
            .artifact_uris
            .nt_catalog_manifest_uri
            .as_deref(),
        Some(persisted_projection.receipt_uri.as_str())
    );
    let persisted_contract_json =
        fs::read_to_string(&artifacts.contract_path).expect("durable contract json");
    let persisted_contract: BacktestResultContract =
        serde_json::from_str(&persisted_contract_json).expect("durable contract parses");
    assert_eq!(
        persisted_contract.catalog_hash, persisted_metadata.catalog_hash,
        "persisted durable contract must keep catalog_hash coherent with persisted metadata"
    );
    assert_eq!(
        persisted_contract
            .artifact_uris
            .nt_catalog_manifest_uri
            .as_deref(),
        Some(persisted_projection.receipt_uri.as_str())
    );
    assert!(
        artifacts
            .persisted_catalog_objects
            .iter()
            .all(|object| object.create_only_write == CreateOnlyWriteDisposition::Created),
        "fresh operator artifact-store run must record create-only object creation"
    );

    let contract_json = fs::read_to_string(&artifacts.contract_path).expect("read contract");
    assert!(
        contract_json.contains(expected_catalog_root.as_str()),
        "durable contract must contain canonical catalog root: {contract_json}"
    );
    assert_store_uri_matches_file(
        &store,
        &artifact_root,
        &persisted_contract.artifact_uris.source_proof_uri,
        &artifacts.proof_path,
    )
    .await;
    assert_store_uri_matches_file(
        &store,
        &artifact_root,
        &persisted_contract.artifact_uris.canonical_table_uri,
        &artifacts.canonical_artifact_path,
    )
    .await;
    assert_store_uri_matches_file(
        &store,
        &artifact_root,
        &persisted_contract.artifact_uris.catalog_metadata_uri,
        &artifacts.catalog_metadata_path,
    )
    .await;
    assert_store_uri_matches_file(
        &store,
        &artifact_root,
        &persisted_contract.artifact_uris.result_contract_uri,
        &artifacts.contract_path,
    )
    .await;
    for object in &artifacts.persisted_catalog_objects {
        let object_path = artifact_root
            .object_path_for_uri(&object.uri)
            .expect("persisted object under artifact root");
        let stored = store
            .get(&object_path)
            .await
            .expect("persisted catalog object")
            .bytes()
            .await
            .expect("persisted catalog bytes");
        assert_eq!(
            u64::try_from(stored.len()).expect("stored catalog byte length"),
            object.byte_len
        );
    }
    let receipt_path = artifact_root
        .object_path_for_uri(&persisted_projection.receipt_uri)
        .expect("operator publication receipt under artifact root");
    let receipt_bytes = store
        .get(&receipt_path)
        .await
        .expect("operator publication receipt")
        .bytes()
        .await
        .expect("operator publication receipt bytes");
    let receipt = CatalogProjectionPublicationReceipt::parse_and_validate_guarded(
        &receipt_bytes,
        &persisted_projection.receipt_sha256,
        &OperatorWorkBudgetGuard::unbounded(),
        OperatorWorkBudgetStage::Publish,
    );
    let receipt = receipt.expect("operator publication receipt validates after restart");
    assert_eq!(receipt.objects.len(), persisted_projection.objects.len());
    assert_eq!(
        receipt.physical_manifest_sha256,
        persisted_projection.physical_manifest_sha256
    );

    let put_count_before_absence_check = store.put_attempts().len();
    let exact_gets_before_absence_check = store.exact_version_get_attempts().len();
    let error = assert_current_durable_completion_absent_with_artifact_store_guarded(
        &spec,
        &store,
        &versioning,
        &source_bindings,
        &OperatorWorkBudgetGuard::unbounded(),
    )
    .await
    .expect_err("fresh execution must reject an existing current terminal");
    assert!(
        format!("{error:#}").contains("refuses existing completion object"),
        "{error:#}"
    );
    assert_eq!(store.put_attempts().len(), put_count_before_absence_check);
    assert_eq!(
        store.exact_version_get_attempts().len(),
        exact_gets_before_absence_check,
        "completion-absence admission must never GET or deserialize terminal bytes"
    );

    let first_candidate_bytes = fs::read(&candidate_path).expect("read first attempt candidate");
    let second_output_dir = tempfile::TempDir::new().expect("second attempt temp dir");
    let second_candidate_path = second_output_dir
        .path()
        .join(OPERATOR_DURABLE_OUTPUT_CANDIDATE_SEAL_FILE);
    let puts_before_rerun = store.put_attempts().len();
    let exact_gets_before_rerun = store.exact_version_get_attempts().len();
    let error = run_from_run_spec_with_artifact_store_guarded(
        &spec,
        gz,
        second_output_dir.path(),
        &store,
        &versioning,
        &OperatorWorkBudgetGuard::unbounded(),
    )
    .await
    .err()
    .expect("operator execution must reject an occupied durable catalog");
    assert!(format!("{error:#}").contains("occupied"), "{error:#}");
    assert_eq!(
        fs::read(&candidate_path).expect("reread first attempt candidate"),
        first_candidate_bytes,
        "a new attempt must not mutate the first immutable local candidate"
    );
    assert!(
        !second_candidate_path.exists(),
        "a rejected fresh execution must not seal a local success candidate"
    );
    assert_eq!(
        store.put_attempts().len(),
        puts_before_rerun + 1,
        "fresh execution issues one strict create attempt and stops on the first occupied object"
    );
    assert_eq!(
        store.exact_version_get_attempts().len(),
        exact_gets_before_rerun,
        "execution must not convert an occupied object into read-side recovery"
    );

    assert_eq!(
        store.exact_version_get_attempts().len(),
        exact_gets_before_absence_check,
        "fresh rerun rejection must not recover through an exact-version GET"
    );
}

#[tokio::test]
async fn operator_completion_lost_ack_is_indeterminate_without_recovery() {
    let gz = gzip(SAMPLE_CSV);
    let spec = committed_run_spec_for(&gz);
    let output_dir = tempfile::TempDir::new().expect("temp dir");
    let artifact_root = spec
        .required_artifact_store()
        .expect("artifact-store config")
        .resolve()
        .expect("artifact root");
    let completion_uri = format!(
        "{}/{}",
        spec.manifest.output_prefix.trim_end_matches('/'),
        DURABLE_COMPLETION_MANIFEST_FILE
    );
    let completion_path = artifact_root
        .object_path_for_uri(&completion_uri)
        .expect("durable completion path");
    let store = VersionedCatalogStore::new().with_lost_put_ack(completion_path.clone());
    let versioning = artifact_root.emulate_bucket_versioning_enabled_for_contract_test();

    let error = run_from_run_spec_with_artifact_store_guarded(
        &spec,
        gz,
        output_dir.path(),
        &store,
        &versioning,
        &OperatorWorkBudgetGuard::unbounded(),
    )
    .await
    .err()
    .expect("lost completion acknowledgement must stop as indeterminate");

    assert!(is_terminal_create_indeterminate(&error), "{error:#}");
    assert!(
        output_dir
            .path()
            .join(OPERATOR_DURABLE_OUTPUT_CANDIDATE_SEAL_FILE)
            .is_file(),
        "the inert local candidate must precede the terminal create attempt"
    );
    assert!(
        store.recorded_version(&completion_path).is_some(),
        "the synthetic store committed before dropping the acknowledgement"
    );
    assert_eq!(
        store
            .put_attempts()
            .iter()
            .filter(|path| *path == &completion_path)
            .count(),
        1,
        "an indeterminate completion create must never be retried"
    );
    assert!(
        store
            .exact_version_get_attempts()
            .iter()
            .all(|(path, _, _)| path != &completion_path),
        "operator execution must not recover an indeterminate completion acknowledgement"
    );
}

#[tokio::test]
async fn conversion_generations_publish_and_discover_independent_terminal_keys() {
    let gz = gzip(SAMPLE_CSV);
    let generation_a = committed_run_spec_for(&gz);
    let mut generation_b = generation_a.spec.clone();
    let bindings_b = generation_b
        .source_bindings_path
        .parent()
        .expect("source bindings have parent")
        .join("source-bindings-generation-b.toml");
    fs::copy(&generation_b.source_bindings_path, &bindings_b)
        .expect("copy identical generation-B source bindings");
    generation_b.source_bindings_path = bindings_b;
    bind_conversion_generation(&mut generation_b);

    assert_ne!(
        generation_a.manifest.output_prefix, generation_b.manifest.output_prefix,
        "the control-artifact identity change must derive a new conversion generation"
    );

    let store = VersionedCatalogStore::new();
    let artifact_root = generation_a
        .required_artifact_store()
        .expect("artifact-store config")
        .resolve()
        .expect("artifact root");
    let versioning = artifact_root.emulate_bucket_versioning_enabled_for_contract_test();
    let output_a = tempfile::tempdir().expect("generation-A output");
    let output_b = tempfile::tempdir().expect("generation-B output");

    let published_a = run_from_run_spec_with_artifact_store_guarded(
        &generation_a,
        gz.clone(),
        output_a.path(),
        &store,
        &versioning,
        &OperatorWorkBudgetGuard::unbounded(),
    )
    .await
    .expect("publish generation A");
    let published_b = run_from_run_spec_with_artifact_store_guarded(
        &generation_b,
        gz.clone(),
        output_b.path(),
        &store,
        &versioning,
        &OperatorWorkBudgetGuard::unbounded(),
    )
    .await
    .expect("publish generation B without colliding with generation A");
    assert_ne!(
        published_a.receipt().completion.object.uri,
        published_b.receipt().completion.object.uri
    );

    for spec in [&generation_a.spec, &generation_b] {
        let registry = VerifiedSourceBindingRegistry::from_run_spec(spec)
            .expect("snapshot generation registry");
        assert_current_durable_completion_absent_with_artifact_store_guarded(
            spec,
            &store,
            &versioning,
            &registry,
            &OperatorWorkBudgetGuard::unbounded(),
        )
        .await
        .expect_err("each published generation must fail the fresh-execution absence check");
    }

    let mut mismatched = generation_b.clone();
    let base = output_prefix_without_conversion_generation(&mismatched.manifest.output_prefix);
    mismatched.manifest.output_prefix = format!(
        "{base}{CONVERSION_GENERATION_PATH_MARKER}{}",
        "f".repeat(64)
    );
    let puts_before = store.put_attempts().len();
    let rejected_output = tempfile::tempdir().expect("mismatched-generation output");
    let error = run_from_run_spec_with_artifact_store_guarded(
        &mismatched,
        gz,
        rejected_output.path(),
        &store,
        &versioning,
        &OperatorWorkBudgetGuard::unbounded(),
    )
    .await
    .err()
    .expect("wrong conversion generation suffix must fail before artifact I/O");
    assert!(format!("{error:#}").contains("conversion generation suffix"));
    assert_eq!(store.put_attempts().len(), puts_before);
    assert!(
        !rejected_output
            .path()
            .join(CONVERSION_MANIFEST_FILE)
            .exists()
    );
}

#[test]
fn terminal_keys_have_one_root_bound_preparation_and_create_route() {
    const ARTIFACT_STORE_SOURCE: &str = include_str!("../src/artifact_store.rs");
    const CAPABILITY_SOURCE: &str = include_str!("../src/nt_catalog_capability.rs");
    const OPERATOR_SOURCE: &str = include_str!("../src/operator.rs");

    assert!(!ARTIFACT_STORE_SOURCE.contains("pub async fn put_create"));
    assert!(!ARTIFACT_STORE_SOURCE.contains("pub async fn put_create_idempotent"));
    assert!(!ARTIFACT_STORE_SOURCE.contains("put_create_uri("));
    assert!(
        !ARTIFACT_STORE_SOURCE
            .contains("pub async fn persist_catalog_projection_for_source_binding(")
    );
    assert!(!ARTIFACT_STORE_SOURCE.contains("pub async fn probe_create_only("));
    assert!(!CAPABILITY_SOURCE.contains("pub fn run_nt_catalog_s3_conformance_probe("));
    assert!(!CAPABILITY_SOURCE.contains("pub fn runtime_evidence("));
    assert!(!CAPABILITY_SOURCE.contains("pub async fn persist_completed_proof_from_evidence("));

    for (label, source) in [
        ("catalog receipt", ARTIFACT_STORE_SOURCE),
        ("capability proof", CAPABILITY_SOURCE),
        ("durable completion", OPERATOR_SOURCE),
    ] {
        assert_eq!(
            source.matches(".prepare_terminal_create_uri(").count(),
            1,
            "{label} must have exactly one root-bound terminal preparation route"
        );
        assert_eq!(
            source.matches(".create_terminal_strict(").count(),
            1,
            "{label} must have exactly one terminal create route"
        );
    }
}

fn sha256(ch: char) -> String {
    std::iter::repeat_n(ch, 64).collect()
}

fn backtest_event(root_uri: String, event_id: &str, artifact_id: &str) -> ArtifactIndexEvent {
    ArtifactIndexEvent {
        schema_version: "artifact-index-event-v1".to_string(),
        created_at: "2026-06-13T00:00:00Z".to_string(),
        event_id: event_id.to_string(),
        artifact_kind: ArtifactKind::Backtests,
        artifact_id: artifact_id.to_string(),
        artifact_uri: format!("{root_uri}result.json"),
        manifest_uri: format!("{root_uri}manifest.json"),
        producer_project: "backtesting-engine".to_string(),
        owner_project: "backtesting-engine".to_string(),
        content_sha256: sha256('a'),
        lifecycle_state: ArtifactLifecycleState::Active,
        storage_profile: ArtifactStorageProfile::Active,
        parent_lineage: vec![ArtifactLineageRef {
            artifact_kind: ArtifactKind::NtCatalog,
            artifact_id: "projection-001".to_string(),
            version: Some("v1".to_string()),
            sha256: sha256('b'),
        }],
        commit_state: ArtifactIndexCommitState::Staged,
    }
}

fn nt_catalog_event(
    root_uri: String,
    event_id: &str,
    artifact_id: &str,
    content_hash_char: char,
) -> ArtifactIndexEvent {
    ArtifactIndexEvent {
        schema_version: "artifact-index-event-v1".to_string(),
        created_at: "2026-06-13T00:00:00Z".to_string(),
        event_id: event_id.to_string(),
        artifact_kind: ArtifactKind::NtCatalog,
        artifact_id: artifact_id.to_string(),
        artifact_uri: format!("{root_uri}catalog-manifest.json"),
        manifest_uri: format!("{root_uri}manifest.json"),
        producer_project: "backtesting-engine".to_string(),
        owner_project: "backtesting-engine".to_string(),
        content_sha256: sha256(content_hash_char),
        lifecycle_state: ArtifactLifecycleState::Active,
        storage_profile: ArtifactStorageProfile::Active,
        parent_lineage: vec![ArtifactLineageRef {
            artifact_kind: ArtifactKind::Raw,
            artifact_id: "raw-001".to_string(),
            version: Some("v1".to_string()),
            sha256: sha256('d'),
        }],
        commit_state: ArtifactIndexCommitState::Staged,
    }
}

fn research_analytics_event(
    root: &ResolvedArtifactRoot,
    subfamily: &str,
    event_id: &str,
    artifact_id: &str,
    content_hash_char: char,
) -> ArtifactIndexEvent {
    ArtifactIndexEvent {
        schema_version: "artifact-index-event-v1".to_string(),
        created_at: "2026-06-13T00:00:00Z".to_string(),
        event_id: event_id.to_string(),
        artifact_kind: ArtifactKind::ResearchAnalytics,
        artifact_id: artifact_id.to_string(),
        artifact_uri: format!(
            "{}/{subfamily}/{artifact_id}/artifact.json",
            root.typed_root(ArtifactKind::ResearchAnalytics)
        ),
        manifest_uri: format!(
            "{}/{subfamily}/{artifact_id}/manifest.json",
            root.typed_root(ArtifactKind::ResearchAnalytics)
        ),
        producer_project: "research-analytics".to_string(),
        owner_project: "research-analytics".to_string(),
        content_sha256: sha256(content_hash_char),
        lifecycle_state: ArtifactLifecycleState::Active,
        storage_profile: ArtifactStorageProfile::Active,
        parent_lineage: vec![ArtifactLineageRef {
            artifact_kind: ArtifactKind::Backtests,
            artifact_id: "backtest-result-001".to_string(),
            version: Some("v1".to_string()),
            sha256: sha256('b'),
        }],
        commit_state: ArtifactIndexCommitState::Staged,
    }
}

fn commit_plan(
    event: ArtifactIndexEvent,
    snapshot_ids: &[&str],
    audit_epoch_id: &str,
) -> ArtifactIndexCommitPlan {
    commit_plan_with_writer(
        event,
        snapshot_ids,
        audit_epoch_id,
        "backtesting-engine-writer",
    )
}

fn commit_plan_with_writer(
    event: ArtifactIndexEvent,
    snapshot_ids: &[&str],
    audit_epoch_id: &str,
    writer_id: &str,
) -> ArtifactIndexCommitPlan {
    ArtifactIndexCommitPlan {
        event,
        snapshot_ids: snapshot_ids
            .iter()
            .map(|snapshot_id| (*snapshot_id).to_string())
            .collect(),
        audit_epoch_ids: vec![audit_epoch_id.to_string()],
        writer_id: writer_id.to_string(),
    }
}

#[tokio::test]
async fn artifact_index_rejects_every_oversize_create_before_store_put() {
    let mut config = artifact_config();
    config.max_final_object_bytes = 8;
    let root = config.resolve().expect("small valid artifact cap");
    let store = S3PreconditionOnCreateConflictStore::new();
    let writer = ArtifactIndexWriter::new(&store);
    let event = backtest_event(
        root.backtest_run_root(MarketStructureFixture::BinaryOption, "oversize-index-run"),
        "oversize-index-event",
        "oversize-index-run",
    );
    let snapshot = ArtifactIndexSnapshot::new(
        "oversize-index-snapshot",
        ArtifactKind::Backtests,
        vec![ArtifactIndexSnapshotRow::from_event(
            &event,
            ArtifactIndexCommitState::Committed,
        )],
    )
    .expect("snapshot is valid");
    let pointer = ArtifactIndexPointer::from_snapshot(&root, &snapshot)
        .expect("pointer derives from snapshot");
    let audit_epoch = ArtifactIndexAuditEpoch {
        audit_epoch_id: "2026-06-13T00:00:21Z".to_string(),
        artifact_kind: ArtifactKind::Backtests,
        prior_snapshot_id: None,
        new_snapshot_id: snapshot.snapshot_id.clone(),
        writer_id: "backtesting-engine-writer".to_string(),
        prior_pointer_e_tag: None,
        new_pointer_e_tag: Some("new-pointer-etag".to_string()),
    };

    for error in [
        writer
            .put_event(&root, &event)
            .await
            .expect_err("oversize event must fail closed"),
        writer
            .put_snapshot(&root, &snapshot)
            .await
            .expect_err("oversize snapshot must fail closed"),
        writer
            .append_audit_epoch(&root, &audit_epoch)
            .await
            .expect_err("oversize audit epoch must fail closed"),
        writer
            .create_latest_pointer(&root, &pointer)
            .await
            .expect_err("oversize latest pointer must fail closed"),
    ] {
        assert!(
            error.to_string().contains("max_final_object_bytes"),
            "{error:#}"
        );
    }
    assert_eq!(
        store.put_attempts(),
        0,
        "oversize artifact-index payload reached the object store"
    );
}

#[tokio::test]
async fn artifact_index_writes_events_snapshots_and_latest_pointer_conditionally() {
    let root = artifact_config().resolve().expect("valid artifact root");
    let store = InMemory::new();
    let writer = ArtifactIndexWriter::new(&store);
    let event = backtest_event(
        root.backtest_run_root(MarketStructureFixture::BinaryOption, "run-001"),
        "event-001",
        "run-001",
    );

    writer
        .put_event(&root, &event)
        .await
        .expect("event create succeeds");
    writer
        .put_event(&root, &event)
        .await
        .expect_err("same event payload must fail strict creation");

    let snapshot = ArtifactIndexSnapshot::new(
        "snapshot-001",
        ArtifactKind::Backtests,
        vec![ArtifactIndexSnapshotRow::from_event(
            &event,
            ArtifactIndexCommitState::Committed,
        )],
    )
    .expect("snapshot is valid");
    assert_eq!(snapshot.rows[0].schema_version, event.schema_version);
    assert_eq!(snapshot.rows[0].owner_project, event.owner_project);
    assert_eq!(
        snapshot.rows[0].lifecycle_state,
        ArtifactLifecycleState::Active
    );
    assert_eq!(
        snapshot.rows[0].storage_profile,
        ArtifactStorageProfile::Active
    );
    writer
        .put_snapshot(&root, &snapshot)
        .await
        .expect("snapshot create succeeds");
    let pointer = ArtifactIndexPointer::from_snapshot(&root, &snapshot)
        .expect("pointer derives from snapshot");
    writer
        .create_latest_pointer(&root, &pointer)
        .await
        .expect("first pointer create succeeds");

    let StoredArtifactIndexPointer {
        pointer: current,
        version: first_version,
    } = writer
        .read_latest_pointer(&root, ArtifactKind::Backtests)
        .await
        .expect("latest pointer reads")
        .expect("latest pointer exists");
    assert_eq!(current.snapshot_id, "snapshot-001");
    assert_eq!(
        current.snapshot_uri,
        "s3://bolt-ra-artifacts/prod/artifact-index/v1/snapshots/kind=backtests/snapshot=snapshot-001.json"
    );

    let next_event = backtest_event(
        root.backtest_run_root(MarketStructureFixture::BinaryOption, "run-002"),
        "event-002",
        "run-002",
    );
    let next_snapshot = ArtifactIndexSnapshot::new(
        "snapshot-002",
        ArtifactKind::Backtests,
        vec![
            ArtifactIndexSnapshotRow::from_event(&event, ArtifactIndexCommitState::Committed),
            ArtifactIndexSnapshotRow::from_event(&next_event, ArtifactIndexCommitState::Committed),
        ],
    )
    .expect("next snapshot is valid");
    writer
        .put_event(&root, &next_event)
        .await
        .expect("next event create succeeds");
    writer
        .put_snapshot(&root, &next_snapshot)
        .await
        .expect("next snapshot create succeeds");
    let next_pointer = ArtifactIndexPointer::from_snapshot(&root, &next_snapshot)
        .expect("next pointer derives from snapshot");

    writer
        .update_latest_pointer(&root, &next_pointer, first_version.clone())
        .await
        .expect("matching pointer version updates");

    let stale_update = writer
        .update_latest_pointer(&root, &pointer, first_version)
        .await
        .expect_err("stale pointer version must fail");
    assert!(
        stale_update.to_string().contains("precondition")
            || stale_update.to_string().contains("does not match"),
        "{stale_update}"
    );
}

#[tokio::test]
async fn artifact_index_snapshot_rejects_staged_rows() {
    let root = artifact_config().resolve().expect("valid artifact root");
    let event = backtest_event(
        root.backtest_run_root(MarketStructureFixture::BinaryOption, "run-002-staged"),
        "event-002-staged",
        "run-002-staged",
    );
    let err = ArtifactIndexSnapshot::new(
        "snapshot-002-staged",
        ArtifactKind::Backtests,
        vec![ArtifactIndexSnapshotRow::from_event(
            &event,
            ArtifactIndexCommitState::Staged,
        )],
    )
    .expect_err("committed snapshot must reject staged rows");

    assert!(err.to_string().contains("committed rows"), "{err}");
}

#[test]
fn artifact_index_event_serialization_requires_lifecycle_and_owner_metadata() {
    let root = artifact_config().resolve().expect("valid artifact root");
    let run_root = root.backtest_run_root(MarketStructureFixture::BinaryOption, "run-002-legacy");
    let legacy_event = serde_json::json!({
        "event_id": "event-002-legacy",
        "artifact_kind": "backtests",
        "artifact_id": "run-002-legacy",
        "artifact_uri": format!("{run_root}result.json"),
        "manifest_uri": format!("{run_root}manifest.json"),
        "producer_project": "backtesting-engine",
        "content_sha256": sha256('a'),
        "parent_lineage": [{
            "artifact_kind": "nt-catalog",
            "artifact_id": "projection-001",
            "version": "v1",
            "sha256": sha256('b')
        }],
        "commit_state": "staged"
    });

    let err = serde_json::from_value::<ArtifactIndexEvent>(legacy_event)
        .expect_err("events without lifecycle and owner metadata must not deserialize");

    assert!(err.to_string().contains("missing field"), "{err}");
}

#[tokio::test]
async fn artifact_index_event_requires_utc_created_at() {
    let root = artifact_config().resolve().expect("valid artifact root");
    let store = InMemory::new();
    let writer = ArtifactIndexWriter::new(&store);
    let mut event = backtest_event(
        root.backtest_run_root(MarketStructureFixture::BinaryOption, "run-002-non-utc"),
        "event-002-non-utc",
        "run-002-non-utc",
    );
    event.created_at = "2026-06-13T09:00:00+09:00".to_string();

    let err = writer
        .put_event(&root, &event)
        .await
        .expect_err("non-UTC artifact index event timestamp must be rejected");

    assert!(err.to_string().contains("created_at must be UTC"), "{err}");
}

#[tokio::test]
async fn artifact_index_reader_rejects_hash_invalid_latest_pointer() {
    let root = artifact_config().resolve().expect("valid artifact root");
    let store = InMemory::new();
    let writer = ArtifactIndexWriter::new(&store);
    let event = backtest_event(
        root.backtest_run_root(MarketStructureFixture::PerpsSpot, "run-003"),
        "event-003",
        "run-003",
    );
    let snapshot = ArtifactIndexSnapshot::new(
        "snapshot-003",
        ArtifactKind::Backtests,
        vec![ArtifactIndexSnapshotRow::from_event(
            &event,
            ArtifactIndexCommitState::Committed,
        )],
    )
    .expect("snapshot is valid");
    writer
        .put_event(&root, &event)
        .await
        .expect("event create succeeds");
    writer
        .put_snapshot(&root, &snapshot)
        .await
        .expect("snapshot create succeeds");

    let mut pointer = ArtifactIndexPointer::from_snapshot(&root, &snapshot)
        .expect("pointer derives from snapshot");
    pointer.snapshot_sha256 = sha256('c');
    writer
        .create_latest_pointer(&root, &pointer)
        .await
        .expect("hash-invalid pointer object can exist");

    let err = writer
        .read_verified_latest_snapshot(&root, ArtifactKind::Backtests)
        .await
        .expect_err("hash-invalid latest pointer must fail closed");
    assert!(err.to_string().contains("snapshot hash"), "{err}");
}

#[tokio::test]
async fn research_analytics_writer_commits_all_owned_families_to_one_kind_snapshot() {
    let root = artifact_config().resolve().expect("valid artifact root");
    let store = InMemory::new();
    let authority = ArtifactIndexWriteAuthority::new(
        "research-analytics-writer",
        [ArtifactKind::ResearchAnalytics],
    )
    .expect("authority config is valid");
    let writer = ArtifactIndexWriter::with_authority(&store, authority);
    let dataset = research_analytics_event(&root, "datasets", "ra-event-001", "dataset-001", 'a');
    let feature_table = research_analytics_event(
        &root,
        "feature-tables",
        "ra-event-002",
        "feature-table-001",
        'c',
    );
    let experiment_result = research_analytics_event(
        &root,
        "experiment-results",
        "ra-event-003",
        "experiment-result-001",
        'd',
    );

    writer
        .commit_event(
            &root,
            commit_plan_with_writer(
                dataset,
                &["snapshot-ra-001"],
                "2026-06-13T00:00:11Z",
                "research-analytics-writer",
            ),
        )
        .await
        .expect("dataset commit succeeds");
    writer
        .commit_event(
            &root,
            commit_plan_with_writer(
                feature_table,
                &["snapshot-ra-002"],
                "2026-06-13T00:00:12Z",
                "research-analytics-writer",
            ),
        )
        .await
        .expect("feature-table commit succeeds");
    writer
        .commit_event(
            &root,
            commit_plan_with_writer(
                experiment_result,
                &["snapshot-ra-003"],
                "2026-06-13T00:00:13Z",
                "research-analytics-writer",
            ),
        )
        .await
        .expect("experiment-results commit succeeds");

    let snapshot = writer
        .read_verified_latest_snapshot(&root, ArtifactKind::ResearchAnalytics)
        .await
        .expect("research analytics latest snapshot verifies");

    assert_eq!(snapshot.artifact_kind, ArtifactKind::ResearchAnalytics);
    assert_eq!(snapshot.rows.len(), 3);
    assert_eq!(
        root.latest_pointer(ArtifactKind::ResearchAnalytics),
        "s3://bolt-ra-artifacts/prod/artifact-index/v1/pointers/kind=research-analytics/latest.json"
    );
    for row in &snapshot.rows {
        assert_eq!(row.artifact_kind, ArtifactKind::ResearchAnalytics);
        assert_eq!(row.producer_project, "research-analytics");
        assert_eq!(row.owner_project, "research-analytics");
        assert_eq!(row.lifecycle_state, ArtifactLifecycleState::Active);
        assert_eq!(row.commit_state, ArtifactIndexCommitState::Committed);
        assert!(
            row.manifest_uri
                .contains("/research-analytics/v1/datasets/")
                || row
                    .manifest_uri
                    .contains("/research-analytics/v1/feature-tables/")
                || row
                    .manifest_uri
                    .contains("/research-analytics/v1/experiment-results/"),
            "{}",
            row.manifest_uri
        );
    }

    let experiment_row = writer
        .read_committed_row(
            &root,
            ArtifactKind::ResearchAnalytics,
            "experiment-result-001",
        )
        .await
        .expect("committed experiment-results row lookup succeeds")
        .expect("committed experiment-results row exists");
    assert_eq!(
        experiment_row.lifecycle_state,
        ArtifactLifecycleState::Active
    );
    assert!(
        experiment_row
            .manifest_uri
            .contains("/research-analytics/v1/experiment-results/")
    );
}

#[tokio::test]
async fn artifact_index_writer_rejects_consumer_mutation_of_research_analytics_records() {
    let root = artifact_config().resolve().expect("valid artifact root");
    let store = InMemory::new();
    let authority = ArtifactIndexWriteAuthority::new("dashboard-writer", [ArtifactKind::Backtests])
        .expect("authority config is valid");
    let writer = ArtifactIndexWriter::with_authority(&store, authority);
    let event = research_analytics_event(
        &root,
        "experiment-results",
        "ra-event-consumer",
        "experiment-result-consumer",
        'e',
    );

    let err = writer
        .commit_event(
            &root,
            commit_plan_with_writer(
                event,
                &["snapshot-ra-consumer"],
                "2026-06-13T00:00:14Z",
                "dashboard-writer",
            ),
        )
        .await
        .expect_err("consumer writer must not mutate RA index records");

    assert!(err.to_string().contains("not authorized"), "{err}");
    assert!(
        writer
            .read_latest_pointer(&root, ArtifactKind::ResearchAnalytics)
            .await
            .expect("latest pointer read succeeds")
            .is_none()
    );
}

#[tokio::test]
async fn research_analytics_index_rejects_promotion_package_family() {
    let root = artifact_config().resolve().expect("valid artifact root");
    let store = InMemory::new();
    let writer = ArtifactIndexWriter::new(&store);
    let event = research_analytics_event(
        &root,
        "promotion-packages",
        "ra-event-promotion-package",
        "promotion-package-001",
        'f',
    );

    let err = writer
        .put_event(&root, &event)
        .await
        .expect_err("promotion-packages is not an RA artifact family");

    assert!(err.to_string().contains("research analytics"), "{err}");
    assert!(err.to_string().contains("experiment-results"), "{err}");
}

#[tokio::test]
async fn artifact_index_rejects_cross_kind_artifact_uri_squatting() {
    let root = artifact_config().resolve().expect("valid artifact root");
    let store = InMemory::new();
    let writer = ArtifactIndexWriter::new(&store);
    let event = backtest_event(
        format!(
            "{}/projection=projection-001/",
            root.typed_root(ArtifactKind::NtCatalog)
        ),
        "event-cross-kind",
        "run-cross-kind",
    );

    let err = writer
        .put_event(&root, &event)
        .await
        .expect_err("backtest event must not claim an NT catalog artifact URI");

    assert!(err.to_string().contains("Backtests"), "{err}");
    assert!(err.to_string().contains("/backtests/v1/"), "{err}");
}

#[tokio::test]
async fn artifact_index_commit_rebases_after_stale_observed_latest() {
    let root = artifact_config().resolve().expect("valid artifact root");
    let store = InMemory::new();
    let writer = ArtifactIndexWriter::new(&store);
    let first = backtest_event(
        root.backtest_run_root(MarketStructureFixture::BinaryOption, "run-010"),
        "event-010",
        "run-010",
    );
    writer
        .commit_event(
            &root,
            commit_plan(first, &["snapshot-010"], "2026-06-13T00:00:00Z"),
        )
        .await
        .expect("initial commit succeeds");
    let stale_observed = writer
        .read_latest_pointer(&root, ArtifactKind::Backtests)
        .await
        .expect("latest pointer reads")
        .expect("latest pointer exists");

    let concurrent = backtest_event(
        root.backtest_run_root(MarketStructureFixture::BinaryOption, "run-011"),
        "event-011",
        "run-011",
    );
    writer
        .commit_event(
            &root,
            commit_plan(concurrent, &["snapshot-011"], "2026-06-13T00:00:01Z"),
        )
        .await
        .expect("concurrent commit succeeds");

    let rebased = backtest_event(
        root.backtest_run_root(MarketStructureFixture::BinaryOption, "run-012"),
        "event-012",
        "run-012",
    );
    let outcome = writer
        .commit_event_from_observed_latest(
            &root,
            commit_plan(
                rebased,
                &["snapshot-012-stale", "snapshot-012-rebased"],
                "2026-06-13T00:00:02Z",
            ),
            Some(stale_observed),
        )
        .await
        .expect("stale observed latest rebases");

    assert_eq!(outcome.snapshot_id, "snapshot-012-rebased");
    assert_eq!(outcome.pointer_attempts, 2);
    assert_eq!(outcome.prior_snapshot_id.as_deref(), Some("snapshot-011"));

    let latest = writer
        .read_verified_latest_snapshot(&root, ArtifactKind::Backtests)
        .await
        .expect("latest snapshot verifies");
    let artifact_ids = latest
        .rows
        .iter()
        .map(|row| row.artifact_id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(artifact_ids, vec!["run-010", "run-011", "run-012"]);
}

#[tokio::test]
async fn artifact_index_commit_appends_audit_epoch() {
    let root = artifact_config().resolve().expect("valid artifact root");
    let store = InMemory::new();
    let writer = ArtifactIndexWriter::new(&store);
    let event = backtest_event(
        root.backtest_run_root(MarketStructureFixture::PerpsSpot, "run-020"),
        "event-020",
        "run-020",
    );
    let outcome = writer
        .commit_event(
            &root,
            commit_plan(event, &["snapshot-020"], "2026-06-13T00:00:03Z"),
        )
        .await
        .expect("commit succeeds");

    assert_eq!(
        outcome.audit_epoch_uri,
        "s3://bolt-ra-artifacts/prod/artifact-index/v1/audit/epochs/2026-06-13T00:00:03Z.json"
    );
    let audit_path = root
        .object_path_for_uri(&outcome.audit_epoch_uri)
        .expect("audit epoch is under artifact root");
    let audit = store
        .get(&audit_path)
        .await
        .expect("audit epoch object")
        .bytes()
        .await
        .expect("audit epoch bytes");
    let audit: serde_json::Value =
        serde_json::from_slice(audit.as_ref()).expect("audit epoch json");
    assert_eq!(audit["artifact_kind"], "backtests");
    assert_eq!(audit["new_snapshot_id"], "snapshot-020");
    assert_eq!(audit["writer_id"], "backtesting-engine-writer");

    let mut conflicting_audit = outcome.audit_epoch.clone();
    conflicting_audit.writer_id.replace_range(..1, "x");
    let original_audit_bytes =
        serde_json::to_vec(&outcome.audit_epoch).expect("serialize original audit epoch");
    let conflicting_audit_bytes =
        serde_json::to_vec(&conflicting_audit).expect("serialize conflicting audit epoch");
    assert_eq!(
        conflicting_audit_bytes.len(),
        original_audit_bytes.len(),
        "content-conflict fixture must preserve payload length"
    );
    assert_ne!(
        conflicting_audit_bytes, original_audit_bytes,
        "content-conflict fixture must change payload bytes"
    );
    let err = writer
        .append_audit_epoch(&root, &conflicting_audit)
        .await
        .expect_err("audit epoch create-only write rejects different payload");
    assert_error_chain_contains(&err, "strict create-only put");
}

#[tokio::test]
async fn artifact_index_keeps_uncommitted_events_out_of_normal_discovery() {
    let root = artifact_config().resolve().expect("valid artifact root");
    let store = InMemory::new();
    let writer = ArtifactIndexWriter::new(&store);
    let committed = backtest_event(
        root.backtest_run_root(MarketStructureFixture::BinaryOption, "run-040"),
        "event-040",
        "run-040",
    );
    writer
        .commit_event(
            &root,
            commit_plan(committed, &["snapshot-040"], "2026-06-13T00:00:05Z"),
        )
        .await
        .expect("committed event reaches latest snapshot");

    let staged = backtest_event(
        root.backtest_run_root(MarketStructureFixture::BinaryOption, "run-041"),
        "event-041",
        "run-041",
    );
    writer
        .put_event(&root, &staged)
        .await
        .expect("staged event can be written as audit input");

    let stored_event = writer
        .read_event(&root, ArtifactKind::Backtests, "event-041")
        .await
        .expect("staged event read succeeds")
        .expect("staged event exists");
    assert_eq!(stored_event.commit_state, ArtifactIndexCommitState::Staged);

    assert!(
        writer
            .read_committed_row(&root, ArtifactKind::Backtests, "run-041")
            .await
            .expect("committed row lookup succeeds")
            .is_none()
    );
    let committed_row = writer
        .read_committed_row(&root, ArtifactKind::Backtests, "run-040")
        .await
        .expect("committed row lookup succeeds")
        .expect("committed row exists");
    assert_eq!(
        committed_row.commit_state,
        ArtifactIndexCommitState::Committed
    );

    let latest = writer
        .read_verified_latest_snapshot(&root, ArtifactKind::Backtests)
        .await
        .expect("latest snapshot verifies");
    let artifact_ids = latest
        .rows
        .iter()
        .map(|row| row.artifact_id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(artifact_ids, vec!["run-040"]);
}

#[tokio::test]
async fn artifact_index_normal_discovery_uses_direct_pointer_reads_without_listing() {
    let root = artifact_config().resolve().expect("valid artifact root");
    let store = NoListObjectStore::new();
    let writer = ArtifactIndexWriter::new(&store);
    let catalog_root = |artifact_id: &str| {
        format!(
            "{}/artifact={artifact_id}/",
            root.typed_root(ArtifactKind::NtCatalog)
        )
    };

    let parent = nt_catalog_event(
        catalog_root("projection-001"),
        "event-060",
        "projection-001",
        'b',
    );
    writer
        .commit_event(
            &root,
            commit_plan(parent, &["snapshot-catalog-060"], "2026-06-13T00:00:09Z"),
        )
        .await
        .expect("parent commits without object-store listing");

    let child = backtest_event(
        root.backtest_run_root(MarketStructureFixture::BinaryOption, "run-060"),
        "event-061",
        "run-060",
    );
    writer
        .commit_event(
            &root,
            commit_plan(child, &["snapshot-backtest-060"], "2026-06-13T00:00:10Z"),
        )
        .await
        .expect("child commits without object-store listing");

    let latest = writer
        .read_verified_latest_snapshot(&root, ArtifactKind::Backtests)
        .await
        .expect("latest snapshot reads without object-store listing");
    let latest_artifact_ids = latest
        .rows
        .iter()
        .map(|row| row.artifact_id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(latest_artifact_ids, vec!["run-060"]);

    let committed_child = writer
        .read_committed_row(&root, ArtifactKind::Backtests, "run-060")
        .await
        .expect("committed row lookup reads without object-store listing")
        .expect("committed child exists");
    assert_eq!(committed_child.artifact_id, "run-060");

    let resolved_parent = writer
        .read_declared_parent_row(
            &root,
            ArtifactKind::Backtests,
            "run-060",
            ArtifactKind::NtCatalog,
            "projection-001",
        )
        .await
        .expect("declared parent lookup reads without object-store listing")
        .expect("declared parent exists");
    assert_eq!(resolved_parent.content_sha256, sha256('b'));
}

#[tokio::test]
async fn artifact_index_parent_lookup_requires_declared_lineage() {
    let root = artifact_config().resolve().expect("valid artifact root");
    let store = InMemory::new();
    let writer = ArtifactIndexWriter::new(&store);
    let catalog_root = |artifact_id: &str| {
        format!(
            "{}/artifact={artifact_id}/",
            root.typed_root(ArtifactKind::NtCatalog)
        )
    };

    let declared_parent = nt_catalog_event(
        catalog_root("projection-001"),
        "event-050",
        "projection-001",
        'b',
    );
    writer
        .commit_event(
            &root,
            commit_plan(
                declared_parent,
                &["snapshot-catalog-050"],
                "2026-06-13T00:00:06Z",
            ),
        )
        .await
        .expect("declared parent commits");

    let child = backtest_event(
        root.backtest_run_root(MarketStructureFixture::BinaryOption, "run-050"),
        "event-051",
        "run-050",
    );
    writer
        .commit_event(
            &root,
            commit_plan(child, &["snapshot-backtest-050"], "2026-06-13T00:00:07Z"),
        )
        .await
        .expect("child commit succeeds");

    let independent_latest = nt_catalog_event(
        catalog_root("projection-002"),
        "event-052",
        "projection-002",
        'c',
    );
    writer
        .commit_event(
            &root,
            commit_plan(
                independent_latest,
                &["snapshot-catalog-052"],
                "2026-06-13T00:00:08Z",
            ),
        )
        .await
        .expect("independent parent commits");

    let resolved = writer
        .read_declared_parent_row(
            &root,
            ArtifactKind::Backtests,
            "run-050",
            ArtifactKind::NtCatalog,
            "projection-001",
        )
        .await
        .expect("declared parent lookup succeeds")
        .expect("declared parent exists");
    assert_eq!(resolved.artifact_id, "projection-001");
    assert_eq!(resolved.content_sha256, sha256('b'));

    let err = writer
        .read_declared_parent_row(
            &root,
            ArtifactKind::Backtests,
            "run-050",
            ArtifactKind::NtCatalog,
            "projection-002",
        )
        .await
        .expect_err("undeclared independent latest parent must be rejected");
    assert!(
        err.to_string().contains("does not declare lineage"),
        "{err}"
    );
}

#[tokio::test]
async fn artifact_index_writer_rejects_consumer_mutation_for_unowned_kind() {
    let root = artifact_config().resolve().expect("valid artifact root");
    let store = InMemory::new();
    let authority = ArtifactIndexWriteAuthority::new(
        "research-analytics-writer",
        [ArtifactKind::ResearchAnalytics],
    )
    .expect("authority config is valid");
    let writer = ArtifactIndexWriter::with_authority(&store, authority);
    let event = backtest_event(
        root.backtest_run_root(MarketStructureFixture::BinaryOption, "run-030"),
        "event-030",
        "run-030",
    );

    let err = writer
        .commit_event(
            &root,
            commit_plan_with_writer(
                event,
                &["snapshot-030"],
                "2026-06-13T00:00:04Z",
                "research-analytics-writer",
            ),
        )
        .await
        .expect_err("consumer writer must not mutate upstream backtest records");

    assert!(err.to_string().contains("not authorized"), "{err}");
    assert!(
        writer
            .read_latest_pointer(&root, ArtifactKind::Backtests)
            .await
            .expect("latest pointer read succeeds")
            .is_none()
    );
}
