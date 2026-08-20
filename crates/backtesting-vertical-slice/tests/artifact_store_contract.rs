use futures_util::{StreamExt, stream::BoxStream};
use object_store::{
    CopyOptions, GetOptions, GetResult, ListResult, MultipartUpload, ObjectMeta, ObjectStore,
    ObjectStoreExt, PutMode, PutMultipartOptions, PutOptions, PutPayload, PutResult,
    Result as ObjectStoreResult, memory::InMemory, path::Path as ObjectPath,
};
use std::{fmt, fs, io::Write, path::Path};

use backtesting_vertical_slice::{
    artifact_store::{
        ArtifactIndexCommitPlan, ArtifactIndexCommitState, ArtifactIndexEvent,
        ArtifactIndexPointer, ArtifactIndexSnapshot, ArtifactIndexSnapshotRow,
        ArtifactIndexWriteAuthority, ArtifactIndexWriter, ArtifactKind, ArtifactLifecycleState,
        ArtifactLineageRef, ArtifactStorageProfile, ArtifactStoreConfig, CatalogDispatchConfig,
        CatalogProjectionBinding, CreateOnlyArtifactWriter, CreateOnlyProbeTranscript,
        CreateOnlyWriteDisposition, ResolvedArtifactRoot, S3ArtifactStoreCredentials,
        StoredArtifactIndexPointer, persist_catalog_projection_for_source_binding,
    },
    conversion_boundary::{CONVERSION_MANIFEST_FILE, ConversionCatalogMetadata},
    nt_catalog_capability::{
        NT_CATALOG_CAPABILITY_PROOF_SCHEMA_VERSION, NtCatalogCapabilityControls,
        NtCatalogCapabilityEvidence, NtCatalogCapabilityProof, NtCatalogCapabilityProofDocument,
        NtCatalogCapabilityRunSpec, NtCatalogCredentialSource, NtCatalogReadBackEvidence,
        SYNTHETIC_SOURCE_PROOF_ID,
    },
    operator::{CATALOG_DIR, RunSpec, run_from_run_spec, run_from_run_spec_with_artifact_store},
    result_contract::BacktestResultContract,
    run_manifest::{ManifestArtifactStoreSsmParameters, MarketStructureFixture},
};
use flate2::{Compression, write::GzEncoder};
use serde::Deserialize;
use sha2::{Digest, Sha256};

const COMMITTED_RUN_SPEC: &str = include_str!(
    "../../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-run-spec.bnbusdc-2026-03-01.toml"
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

fn expected_catalog_projection_manifest_sha256(entries: &[(&str, usize, &str)]) -> String {
    let mut lines = entries
        .iter()
        .map(|(relative_path, byte_len, sha256)| format!("{relative_path}\t{byte_len}\t{sha256}\n"))
        .collect::<Vec<_>>();
    lines.sort();
    sha256_hex(lines.concat().as_bytes())
}

fn committed_run_spec_for(gz_bytes: &[u8]) -> RunSpec {
    let mut spec: RunSpec = toml::from_str(COMMITTED_RUN_SPEC).expect("run-spec parses");
    let object_hash = sha256_hex(gz_bytes);
    spec.accepted_object.sha256 = object_hash.clone();
    spec.accepted_object.bytes = gz_bytes.len() as u64;
    spec.source_proof.raw_sample_hash = object_hash;
    spec
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
    toml::from_str(COMMITTED_RUN_SPEC).expect("run-spec capability proof parses")
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
catalog_projection_manifest_object = "catalog-projection-manifest.json"

[s3]
region = "us-east-1"
conditional_put = "etag"
copy_if_not_exists = "multipart"

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
}

impl S3PreconditionOnCreateConflictStore {
    fn new() -> Self {
        Self {
            inner: InMemory::new(),
        }
    }
}

impl fmt::Display for S3PreconditionOnCreateConflictStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("S3PreconditionOnCreateConflictStore")
    }
}

#[async_trait::async_trait]
impl ObjectStore for S3PreconditionOnCreateConflictStore {
    async fn put_opts(
        &self,
        location: &ObjectPath,
        payload: PutPayload,
        opts: PutOptions,
    ) -> ObjectStoreResult<PutResult> {
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

#[derive(Debug)]
struct FailAuditObjectStore {
    inner: InMemory,
}

impl FailAuditObjectStore {
    fn new() -> Self {
        Self {
            inner: InMemory::new(),
        }
    }
}

impl fmt::Display for FailAuditObjectStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("FailAuditObjectStore")
    }
}

#[async_trait::async_trait]
impl ObjectStore for FailAuditObjectStore {
    async fn put_opts(
        &self,
        location: &ObjectPath,
        payload: PutPayload,
        opts: PutOptions,
    ) -> ObjectStoreResult<PutResult> {
        if location
            .as_ref()
            .contains("artifact-index/v1/audit/intents/v1/")
        {
            return Err(object_store::Error::Generic {
                store: "FailAuditObjectStore",
                source: Box::new(std::io::Error::other("synthetic audit failure")),
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
async fn create_only_idempotent_replay_accepts_s3_precondition_for_same_payload() {
    let store = S3PreconditionOnCreateConflictStore::new();
    let writer = CreateOnlyArtifactWriter::new(&store);
    let path = ObjectPath::from("artifact-index/v1/events/kind=backtests/event=event-001.json");
    let payload = br#"{"event_id":"event-001"}"#.to_vec();

    let (_version, disposition) = writer
        .put_create_idempotent_with_disposition(&path, payload.clone())
        .await
        .expect("first create succeeds");
    assert_eq!(disposition, CreateOnlyWriteDisposition::Created);

    let (_version, disposition) = writer
        .put_create_idempotent_with_disposition(&path, payload)
        .await
        .expect("S3 precondition conflict with same payload is idempotent");
    assert_eq!(
        disposition,
        CreateOnlyWriteDisposition::AlreadyExistedSamePayload
    );
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
    let store = InMemory::new();
    let writer = CreateOnlyArtifactWriter::new(&store);
    let persisted = fixture
        .nt_catalog_capability_proof
        .persist_completed_proof_from_evidence(&fixture.artifact_store, &writer, &evidence)
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
    persisted
        .proof
        .direct_s3_catalog_access_proven(&committed_root)
        .expect("persisted proof validates");
    let persisted_path = committed_root
        .object_path_for_uri(&persisted.proof_artifact_uri)
        .expect("proof artifact uri is under artifact root");
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
    let idempotent_persisted = fixture
        .nt_catalog_capability_proof
        .persist_completed_proof_from_evidence(&fixture.artifact_store, &writer, &evidence)
        .await
        .expect("same proof artifact bytes are idempotent");
    assert_eq!(
        idempotent_persisted.proof_artifact_create_only_write,
        CreateOnlyWriteDisposition::AlreadyExistedSamePayload
    );
    assert_eq!(
        idempotent_persisted.proof_artifact_sha256,
        persisted.proof_artifact_sha256
    );
    let mut changed_valid_evidence = evidence.clone();
    changed_valid_evidence.read_back.query_files_result_count += 1;
    let err = fixture
        .nt_catalog_capability_proof
        .persist_completed_proof_from_evidence(
            &fixture.artifact_store,
            &writer,
            &changed_valid_evidence,
        )
        .await
        .expect_err("changed proof artifact bytes must be rejected at the same URI");
    assert!(
        err.to_string().contains("different payload"),
        "expected different-payload create-only error, got {err:#}"
    );

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

#[test]
fn rejects_local_or_non_s3_canonical_artifact_roots() {
    let mut config = artifact_config();
    config.artifact_root = "/tmp/not-canonical".to_string();
    assert!(config.resolve().is_err());

    config.artifact_root = "file:///tmp/not-canonical".to_string();
    assert!(config.resolve().is_err());
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
    let writer = CreateOnlyArtifactWriter::new(&store);

    let transcript = writer
        .probe_create_only(&root, "probe-run-123")
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
async fn create_only_probe_replays_existing_same_payload_sentinels() {
    let root = artifact_config().resolve().expect("valid artifact root");
    let store = InMemory::new();
    let writer = CreateOnlyArtifactWriter::new(&store);

    let first = writer
        .probe_create_only(&root, "probe-run-123")
        .await
        .expect("first create-only probe");
    let replay = writer
        .probe_create_only(&root, "probe-run-123")
        .await
        .expect("same-payload probe replay must be idempotent");

    assert_eq!(replay.probe_uri, first.probe_uri);
    assert_eq!(replay.copy_source_uri, first.copy_source_uri);
    assert_eq!(replay.copy_dest_uri, first.copy_dest_uri);
    assert!(replay.first_create_succeeded);
    assert!(replay.duplicate_create_rejected);
    assert!(replay.first_copy_succeeded);
    assert!(replay.duplicate_copy_rejected);
}

#[tokio::test]
async fn persists_catalog_projection_directory_with_create_only_dispatch() {
    let root = artifact_config().resolve().expect("valid artifact root");
    let dispatch = CatalogDispatchConfig {
        bindings: vec![CatalogProjectionBinding {
            source_binding: "binary-official".to_string(),
            market_structure_fixture: MarketStructureFixture::BinaryOption,
            catalog_projection_id: "projection-run-123".to_string(),
        }],
    };
    let temp = tempfile::TempDir::new().expect("temp dir");
    let nested_dir = temp
        .path()
        .join("data/trade_tick/instrument=BTC-USD.BINARY");
    fs::create_dir_all(&nested_dir).expect("catalog directory");
    fs::write(temp.path().join("metadata.json"), br#"{"schema":"nt"}"#).expect("metadata");
    fs::write(nested_dir.join("part-000.parquet"), b"trade-ticks").expect("catalog data");

    let store = InMemory::new();
    let persisted = persist_catalog_projection_for_source_binding(
        &store,
        &root,
        &dispatch,
        "binary-official",
        MarketStructureFixture::BinaryOption,
        temp.path(),
    )
    .await
    .expect("catalog persisted");

    assert_eq!(
        persisted.catalog_root_uri,
        "s3://bolt-ra-artifacts/prod/nt-catalog/v1/projection=projection-run-123/"
    );
    assert_eq!(persisted.binding.source_binding, "binary-official");
    assert_eq!(
        persisted.binding.market_structure_fixture,
        MarketStructureFixture::BinaryOption
    );
    assert_eq!(
        persisted.binding.catalog_projection_id,
        "projection-run-123"
    );
    assert_eq!(persisted.objects.len(), 2);
    let metadata_sha256 = sha256_hex(br#"{"schema":"nt"}"#);
    let catalog_sha256 = sha256_hex(b"trade-ticks");
    assert_eq!(
        persisted.manifest_uri,
        "s3://bolt-ra-artifacts/prod/nt-catalog/v1/projection=projection-run-123/catalog-projection-manifest.json"
    );
    assert_eq!(
        persisted.manifest_create_only_write,
        CreateOnlyWriteDisposition::Created
    );
    assert_eq!(
        persisted.manifest_sha256,
        expected_catalog_projection_manifest_sha256(&[
            (
                "data/trade_tick/instrument=BTC-USD.BINARY/part-000.parquet",
                b"trade-ticks".len(),
                &catalog_sha256
            ),
            (
                "metadata.json",
                br#"{"schema":"nt"}"#.len(),
                &metadata_sha256
            ),
        ]),
        "catalog projection manifest hash must be derived from sorted relative path, size, and object hash"
    );
    assert!(
        persisted
            .objects
            .iter()
            .all(|object| object.create_only_write == CreateOnlyWriteDisposition::Created),
        "first catalog projection persist must record create-only object creation"
    );
    let manifest_object_path = root
        .object_path_for_uri(&persisted.manifest_uri)
        .expect("manifest under artifact root");
    let manifest_bytes = store
        .get(&manifest_object_path)
        .await
        .expect("projection manifest object")
        .bytes()
        .await
        .expect("projection manifest bytes");
    let manifest_json: serde_json::Value =
        serde_json::from_slice(&manifest_bytes).expect("projection manifest json");
    assert_eq!(
        manifest_json["schema_version"].as_str(),
        Some("catalog-projection-manifest-v1")
    );
    assert_eq!(
        manifest_json["manifest_sha256"].as_str(),
        Some(persisted.manifest_sha256.as_str())
    );
    assert_eq!(
        manifest_json["binding"]["source_binding"].as_str(),
        Some(persisted.binding.source_binding.as_str())
    );
    let manifest_objects = manifest_json["objects"]
        .as_array()
        .expect("projection manifest objects array");
    assert!(
        manifest_objects
            .iter()
            .all(|object| object.get("create_only_write").is_none()),
        "projection manifest bytes must not include per-run create-only dispositions"
    );
    assert!(
        persisted
            .objects
            .iter()
            .any(|object| object.uri.ends_with("/metadata.json"))
    );
    let catalog_object = persisted
        .objects
        .iter()
        .find(|object| object.uri.ends_with("/part-000.parquet"))
        .expect("catalog parquet object");
    assert_eq!(
        catalog_object.relative_path,
        "data/trade_tick/instrument=BTC-USD.BINARY/part-000.parquet"
    );
    let object_path = root
        .object_path_for_uri(&catalog_object.uri)
        .expect("uri under artifact root");
    let stored = store
        .get(&object_path)
        .await
        .expect("created catalog object")
        .bytes()
        .await
        .expect("catalog object bytes");
    assert_eq!(stored.as_ref(), b"trade-ticks");
    assert_eq!(catalog_object.byte_len, b"trade-ticks".len());
    assert_eq!(
        catalog_object.create_only_write,
        CreateOnlyWriteDisposition::Created
    );
}

#[cfg(unix)]
#[tokio::test]
async fn rejects_catalog_projection_symlink_without_following() {
    let root = artifact_config().resolve().expect("valid artifact root");
    let dispatch = CatalogDispatchConfig {
        bindings: vec![CatalogProjectionBinding {
            source_binding: "binary-official".to_string(),
            market_structure_fixture: MarketStructureFixture::BinaryOption,
            catalog_projection_id: "projection-run-123".to_string(),
        }],
    };
    let temp = tempfile::TempDir::new().expect("temp dir");
    let outside = tempfile::TempDir::new().expect("outside dir");
    fs::create_dir_all(outside.path().join("data/trade_tick")).expect("outside catalog dir");
    fs::write(
        outside.path().join("data/trade_tick/part-000.parquet"),
        b"outside-root",
    )
    .expect("outside catalog data");
    std::os::unix::fs::symlink(outside.path(), temp.path().join("linked-catalog"))
        .expect("catalog symlink");

    let store = InMemory::new();
    let err = persist_catalog_projection_for_source_binding(
        &store,
        &root,
        &dispatch,
        "binary-official",
        MarketStructureFixture::BinaryOption,
        temp.path(),
    )
    .await
    .expect_err("catalog projection must reject symlinks instead of following them");

    assert!(
        format!("{err:#}").contains("catalog projection contains non-regular file linked-catalog"),
        "{err:#}"
    );
}

#[tokio::test]
async fn rejects_duplicate_catalog_projection_bytes() {
    let root = artifact_config().resolve().expect("valid artifact root");
    let dispatch = CatalogDispatchConfig {
        bindings: vec![CatalogProjectionBinding {
            source_binding: "binary-official".to_string(),
            market_structure_fixture: MarketStructureFixture::BinaryOption,
            catalog_projection_id: "projection-run-123".to_string(),
        }],
    };
    let temp = tempfile::TempDir::new().expect("temp dir");
    let catalog_file = temp.path().join("data/trade_tick/part-000.parquet");
    fs::create_dir_all(catalog_file.parent().expect("parent")).expect("catalog directory");
    fs::write(&catalog_file, b"first").expect("first catalog data");

    let store = InMemory::new();
    let first = persist_catalog_projection_for_source_binding(
        &store,
        &root,
        &dispatch,
        "binary-official",
        MarketStructureFixture::BinaryOption,
        temp.path(),
    )
    .await
    .expect("first catalog persist");
    let idempotent = persist_catalog_projection_for_source_binding(
        &store,
        &root,
        &dispatch,
        "binary-official",
        MarketStructureFixture::BinaryOption,
        temp.path(),
    )
    .await
    .expect("same catalog bytes are idempotent");
    assert_eq!(
        idempotent.manifest_sha256, first.manifest_sha256,
        "same catalog bytes must produce the same sorted projection manifest hash"
    );
    assert_eq!(
        idempotent.manifest_create_only_write,
        CreateOnlyWriteDisposition::AlreadyExistedSamePayload,
        "same manifest bytes must be an idempotent create-only replay"
    );
    assert!(
        idempotent
            .objects
            .iter()
            .all(|object| object.create_only_write
                == CreateOnlyWriteDisposition::AlreadyExistedSamePayload),
        "same-payload create-only conflicts must be recorded as idempotent, not rewritten"
    );
    fs::write(&catalog_file, b"second").expect("second catalog data");

    let err = persist_catalog_projection_for_source_binding(
        &store,
        &root,
        &dispatch,
        "binary-official",
        MarketStructureFixture::BinaryOption,
        temp.path(),
    )
    .await
    .expect_err("duplicate projection bytes must be rejected");

    assert!(format!("{err:#}").contains("different payload"), "{err:#}");
}

#[tokio::test]
async fn rejects_catalog_dispatch_fixture_mismatch() {
    let root = artifact_config().resolve().expect("valid artifact root");
    let dispatch = CatalogDispatchConfig {
        bindings: vec![CatalogProjectionBinding {
            source_binding: "binary-official".to_string(),
            market_structure_fixture: MarketStructureFixture::BinaryOption,
            catalog_projection_id: "projection-run-123".to_string(),
        }],
    };
    let temp = tempfile::TempDir::new().expect("temp dir");
    let catalog_file = temp.path().join("data/trade_tick/part-000.parquet");
    fs::create_dir_all(catalog_file.parent().expect("parent")).expect("catalog directory");
    fs::write(&catalog_file, b"fixture-mismatch").expect("catalog data");

    let store = InMemory::new();
    let err = persist_catalog_projection_for_source_binding(
        &store,
        &root,
        &dispatch,
        "binary-official",
        MarketStructureFixture::PerpsSpot,
        temp.path(),
    )
    .await
    .expect_err("market_structure_fixture mismatch must reject catalog persistence");

    assert!(
        err.to_string()
            .contains("market_structure_fixture mismatch"),
        "{err}"
    );
}

#[test]
fn rejects_manifest_fixture_mismatch() {
    let gz = gzip(SAMPLE_CSV);
    let mut spec = committed_run_spec_for(&gz);
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

    let err = match run_from_run_spec_with_artifact_store(
        &spec,
        &gz,
        output_dir.path(),
        &store,
        |_, _, _| panic!("artifact-store region mismatch must fail before runtime evidence"),
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

    let err = match run_from_run_spec_with_artifact_store(
        &spec,
        &gz,
        output_dir.path(),
        &store,
        |_, _, _| panic!("artifact-store SSM region mismatch must fail before runtime evidence"),
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
    let gz = gzip(SAMPLE_CSV);
    let spec = committed_run_spec_for(&gz);
    let output_dir = tempfile::TempDir::new().expect("temp dir");
    let store = InMemory::new();
    let artifact_store = spec
        .required_artifact_store()
        .expect("artifact-store config");
    let catalog_dispatch = spec
        .required_catalog_dispatch()
        .expect("catalog dispatch config");
    let nt_catalog_capability_proof = spec
        .required_nt_catalog_capability_proof()
        .expect("NT catalog capability proof config");
    let artifact_root = artifact_store.resolve().expect("artifact root resolves");
    let expected_catalog_root = catalog_dispatch
        .catalog_root_for(
            &spec.source_proof.source_binding,
            spec.manifest.market_structure_fixture,
            &artifact_root,
        )
        .expect("source binding dispatches");

    let artifacts = run_from_run_spec_with_artifact_store(
        &spec,
        &gz,
        output_dir.path(),
        &store,
        |artifact_root, plan, create_only_probe| {
            let mut evidence =
                successful_capability_evidence(artifact_root, nt_catalog_capability_proof);
            evidence.read_back.catalog_uri = plan.synthetic_catalog_root_uri.clone();
            evidence.nt_catalog_storage_option_keys = plan.storage_options_keys.clone();
            evidence.create_only_probe = create_only_probe;
            Ok(evidence)
        },
    )
    .await
    .expect("operator artifact-store run");

    assert_eq!(
        artifacts.canonical_catalog_uri.as_deref(),
        Some(expected_catalog_root.as_str())
    );
    assert!(
        !artifacts.catalog_root.exists(),
        "artifact-store path must remove the transient local NT catalog after durable persistence"
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
        artifacts
            .create_only_probe_transcript
            .as_ref()
            .expect("create-only probe transcript")
            .duplicate_create_rejected
    );
    assert!(
        artifacts
            .create_only_probe_transcript
            .as_ref()
            .expect("create-only probe transcript")
            .duplicate_copy_rejected
    );
    let nt_catalog_capability_plan = artifacts
        .nt_catalog_capability_plan
        .as_ref()
        .expect("NT catalog capability proof plan");
    assert_eq!(
        nt_catalog_capability_plan.synthetic_catalog_root_uri,
        "s3://bolt-parquet/nt-research-analytics/nt-catalog-synthetic-proof/v1/proof=synthetic-capability-proof/"
    );
    assert_eq!(
        nt_catalog_capability_plan.proof_artifact_uri,
        "s3://bolt-parquet/nt-research-analytics/nt-catalog-synthetic-proof/v1/proof=synthetic-capability-proof/nt-catalog-capability-proof.json"
    );
    assert_eq!(
        nt_catalog_capability_plan.storage_options_keys,
        vec!["region".to_string()]
    );
    let proof_artifact = artifacts
        .nt_catalog_capability_proof_artifact
        .as_ref()
        .expect("operator must persist NT catalog capability proof artifact");
    assert_eq!(
        proof_artifact.proof_artifact_uri,
        nt_catalog_capability_plan.proof_artifact_uri
    );
    assert_eq!(
        proof_artifact.proof_artifact_create_only_write,
        CreateOnlyWriteDisposition::Created
    );
    assert_eq!(
        proof_artifact.evidence.create_only_probe,
        *artifacts
            .create_only_probe_transcript
            .as_ref()
            .expect("create-only probe transcript")
    );
    assert_eq!(
        proof_artifact.evidence.read_back.catalog_uri,
        nt_catalog_capability_plan.synthetic_catalog_root_uri
    );
    let proof_artifact_path = artifact_store
        .resolve()
        .expect("artifact root")
        .object_path_for_uri(&proof_artifact.proof_artifact_uri)
        .expect("proof artifact path");
    let proof_artifact_bytes = store
        .get(&proof_artifact_path)
        .await
        .expect("proof artifact exists")
        .bytes()
        .await
        .expect("proof artifact bytes");
    assert_eq!(
        sha256_hex(proof_artifact_bytes.as_ref()),
        proof_artifact.proof_artifact_sha256
    );
    let proof_document: NtCatalogCapabilityProofDocument =
        serde_json::from_slice(proof_artifact_bytes.as_ref()).expect("proof document parses");
    assert_eq!(proof_document.proof, proof_artifact.proof);
    assert_eq!(proof_document.evidence, proof_artifact.evidence);
    assert!(
        !artifacts.persisted_catalog_objects.is_empty(),
        "operator must persist projected catalog objects through artifact-store dispatch"
    );
    let persisted_projection = artifacts
        .persisted_catalog_projection
        .as_ref()
        .expect("operator must expose persisted catalog projection proof");
    assert_eq!(
        persisted_projection.manifest_create_only_write,
        CreateOnlyWriteDisposition::Created
    );
    assert_eq!(
        persisted_projection.binding.source_binding,
        spec.source_proof.source_binding
    );
    assert_eq!(
        persisted_projection.manifest_sha256,
        expected_catalog_projection_manifest_sha256(
            &persisted_projection
                .objects
                .iter()
                .map(|object| {
                    (
                        object.relative_path.as_str(),
                        object.byte_len,
                        object.sha256.as_str(),
                    )
                })
                .collect::<Vec<_>>()
        )
    );
    assert_eq!(
        artifacts.output.contract.catalog_hash,
        artifacts.output.conversion_catalog_metadata.catalog_hash,
        "durable result contract must keep catalog_hash coherent with catalog metadata"
    );
    assert_eq!(
        artifacts
            .output
            .conversion_catalog_metadata
            .execution_catalog_uri,
        expected_catalog_root,
        "artifact-store path must rewrite catalog metadata to the durable catalog root"
    );
    assert!(
        artifacts
            .output
            .conversion_catalog_metadata
            .direct_s3_catalog_access_proven,
        "artifact-store path must record the proved direct-S3 catalog access"
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
        Some(persisted_projection.manifest_uri.as_str())
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
        Some(persisted_projection.manifest_uri.as_str())
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
        assert_eq!(stored.len(), object.byte_len);
    }
    let manifest_path = artifact_root
        .object_path_for_uri(&persisted_projection.manifest_uri)
        .expect("operator projection manifest under artifact root");
    let manifest_bytes = store
        .get(&manifest_path)
        .await
        .expect("operator projection manifest")
        .bytes()
        .await
        .expect("operator projection manifest bytes");
    assert!(
        serde_json::from_slice::<serde_json::Value>(&manifest_bytes)
            .expect("operator projection manifest json")["objects"]
            .as_array()
            .expect("objects array")
            .len()
            == persisted_projection.objects.len()
    );

    let mut second_spec = spec.clone();
    second_spec.create_only_probe_id =
        Some("backtesting-vertical-slice-bnbusdc-2026-03-01-rerun".to_string());
    second_spec
        .nt_catalog_capability_proof
        .as_mut()
        .expect("run spec carries NT catalog capability proof")
        .proof_run_id = "synthetic-capability-proof-rerun".to_string();
    let second = run_from_run_spec_with_artifact_store(
        &second_spec,
        &gz,
        output_dir.path(),
        &store,
        |artifact_root, plan, create_only_probe| {
            let mut evidence =
                successful_capability_evidence(artifact_root, nt_catalog_capability_proof);
            evidence.read_back.catalog_uri = plan.synthetic_catalog_root_uri.clone();
            evidence.nt_catalog_storage_option_keys = plan.storage_options_keys.clone();
            evidence.create_only_probe = create_only_probe;
            Ok(evidence)
        },
    )
    .await
    .expect("operator artifact-store rerun replays idempotently");
    assert_eq!(
        second.canonical_catalog_uri.as_deref(),
        Some(expected_catalog_root.as_str())
    );
    assert!(
        !second.catalog_root.exists(),
        "artifact-store rerun should still remove the transient local NT catalog after durable persistence"
    );
    assert_eq!(
        second
            .persisted_catalog_projection
            .as_ref()
            .expect("second persisted projection")
            .manifest_create_only_write,
        CreateOnlyWriteDisposition::AlreadyExistedSamePayload
    );
    assert!(
        second
            .persisted_catalog_objects
            .iter()
            .all(|object| object.create_only_write
                == CreateOnlyWriteDisposition::AlreadyExistedSamePayload),
        "artifact-store rerun must idempotently reuse durable catalog objects"
    );
    assert_eq!(
        second
            .nt_catalog_capability_proof_artifact
            .as_ref()
            .expect("second proof artifact")
            .proof_artifact_create_only_write,
        CreateOnlyWriteDisposition::Created
    );
    let second_contract: BacktestResultContract = serde_json::from_str(
        &fs::read_to_string(&second.contract_path).expect("second durable contract json"),
    )
    .expect("second durable contract parses");
    assert_eq!(
        second_contract
            .artifact_uris
            .nt_catalog_manifest_uri
            .as_deref(),
        Some(persisted_projection.manifest_uri.as_str())
    );
}

#[tokio::test]
async fn create_only_writer_refuses_to_overwrite_existing_object() {
    let root = artifact_config().resolve().expect("valid artifact root");
    let store = InMemory::new();
    let writer = CreateOnlyArtifactWriter::new(&store);
    let object_uri =
        root.backtest_run_root(MarketStructureFixture::PerpsSpot, "run-123") + "result.json";
    let object_path = root
        .object_path_for_uri(&object_uri)
        .expect("uri under artifact root");

    writer
        .put_create_uri(&root, &object_uri, br#"{"status":"first"}"#.to_vec())
        .await
        .expect("first create succeeds");
    let err = writer
        .put_create_uri(&root, &object_uri, br#"{"status":"second"}"#.to_vec())
        .await
        .expect_err("second create must fail");
    assert!(format!("{err:#}").contains("already exists"), "{err:#}");

    let stored = store
        .get(&object_path)
        .await
        .expect("created object")
        .bytes()
        .await
        .expect("object bytes");
    assert_eq!(stored.as_ref(), br#"{"status":"first"}"#);

    assert!(
        writer
            .put_create_uri(
                &root,
                "s3://other-bucket/prod/backtests/v1/run=run-123/result.json",
                br#"{"status":"outside"}"#.to_vec(),
            )
            .await
            .is_err()
    );
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

fn commit_plan(event: ArtifactIndexEvent, snapshot_ids: &[&str]) -> ArtifactIndexCommitPlan {
    commit_plan_with_writer(event, snapshot_ids, "backtesting-engine-writer")
}

fn commit_plan_with_writer(
    event: ArtifactIndexEvent,
    snapshot_ids: &[&str],
    writer_id: &str,
) -> ArtifactIndexCommitPlan {
    ArtifactIndexCommitPlan {
        event,
        snapshot_ids: snapshot_ids
            .iter()
            .map(|snapshot_id| (*snapshot_id).to_string())
            .collect(),
        writer_id: writer_id.to_string(),
    }
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
        .expect("same event payload is idempotent");

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
    let event_wire = serde_json::to_value(&event).expect("event wire JSON");
    let row_wire = serde_json::to_value(&snapshot.rows[0]).expect("snapshot row wire JSON");
    for wire in [&event_wire, &row_wire] {
        assert!(wire.get("artifact_subfamily").is_none());
        assert!(wire.get("domain_state").is_none());
    }
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
    let experiment_contract = research_analytics_event(
        &root,
        "experiment-contracts",
        "ra-event-004",
        "experiment-contract-001",
        'e',
    );
    writer
        .commit_event(
            &root,
            commit_plan_with_writer(dataset, &["snapshot-ra-001"], "research-analytics-writer"),
        )
        .await
        .expect("dataset commit succeeds");
    writer
        .commit_event(
            &root,
            commit_plan_with_writer(
                feature_table,
                &["snapshot-ra-002"],
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
                "research-analytics-writer",
            ),
        )
        .await
        .expect("experiment-results commit succeeds");
    writer
        .commit_event(
            &root,
            commit_plan_with_writer(
                experiment_contract,
                &["snapshot-ra-004"],
                "research-analytics-writer",
            ),
        )
        .await
        .expect("experiment-contract commit succeeds");

    let snapshot = writer
        .read_verified_latest_snapshot(&root, ArtifactKind::ResearchAnalytics)
        .await
        .expect("research analytics latest snapshot verifies");

    assert_eq!(snapshot.artifact_kind, ArtifactKind::ResearchAnalytics);
    assert_eq!(snapshot.rows.len(), 4);
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
                    .contains("/research-analytics/v1/experiment-results/")
                || row
                    .manifest_uri
                    .contains("/research-analytics/v1/experiment-contracts/"),
            "{}",
            row.manifest_uri
        );
    }
    let contract = snapshot
        .rows
        .iter()
        .find(|row| row.artifact_id == "experiment-contract-001")
        .expect("experiment contract row");
    assert!(
        contract
            .artifact_uri
            .contains("/research-analytics/v1/experiment-contracts/")
    );

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
            commit_plan_with_writer(event, &["snapshot-ra-consumer"], "dashboard-writer"),
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
    let first_outcome = writer
        .commit_event(&root, commit_plan(first, &["snapshot-010"]))
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
    let concurrent_outcome = writer
        .commit_event(&root, commit_plan(concurrent, &["snapshot-011"]))
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
            commit_plan(rebased, &["snapshot-012-stale", "snapshot-012-rebased"]),
            Some(stale_observed),
        )
        .await
        .expect("stale observed latest rebases");

    assert_eq!(outcome.snapshot_id, "snapshot-012-rebased");
    assert_eq!(outcome.pointer_attempts, 2);
    assert_eq!(outcome.prior_snapshot_id.as_deref(), Some("snapshot-011"));
    assert_eq!(outcome.audit_intent.new_snapshot_id, "snapshot-012-rebased");
    assert_eq!(
        outcome.audit_intent.prior_snapshot_id.as_deref(),
        Some("snapshot-011")
    );
    assert_ne!(
        first_outcome.audit_intent.audit_intent_id,
        concurrent_outcome.audit_intent.audit_intent_id
    );
    assert_ne!(
        concurrent_outcome.audit_intent.audit_intent_id,
        outcome.audit_intent.audit_intent_id
    );

    let audit_prefix = ObjectPath::from("prod/artifact-index/v1/audit/intents/v1/kind=backtests");
    let audit_objects = store.list(Some(&audit_prefix)).collect::<Vec<_>>().await;
    assert_eq!(audit_objects.len(), 4);
    assert!(audit_objects.iter().all(Result::is_ok));

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
async fn artifact_index_commit_prewrites_versioned_audit_intent() {
    let root = artifact_config().resolve().expect("valid artifact root");
    let store = InMemory::new();
    let writer = ArtifactIndexWriter::new(&store);
    let event = backtest_event(
        root.backtest_run_root(MarketStructureFixture::PerpsSpot, "run-020"),
        "event-020",
        "run-020",
    );
    let outcome = writer
        .commit_event(&root, commit_plan(event, &["snapshot-020"]))
        .await
        .expect("commit succeeds");

    assert_eq!(outcome.audit_intent.audit_intent_id.len(), 64);
    assert!(
        outcome
            .audit_intent
            .audit_intent_id
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    );
    assert_eq!(
        outcome.audit_intent_uri,
        format!(
            "s3://bolt-ra-artifacts/prod/artifact-index/v1/audit/intents/v1/kind=backtests/{}.json",
            outcome.audit_intent.audit_intent_id
        )
    );
    let audit_path = root
        .object_path_for_uri(&outcome.audit_intent_uri)
        .expect("audit intent is under artifact root");
    let audit = store
        .get(&audit_path)
        .await
        .expect("audit intent object")
        .bytes()
        .await
        .expect("audit intent bytes");
    let audit: serde_json::Value =
        serde_json::from_slice(audit.as_ref()).expect("audit intent json");
    assert_eq!(audit["schema_version"], "artifact-index-audit-intent.v1");
    assert_eq!(audit["artifact_kind"], "backtests");
    assert_eq!(audit["new_snapshot_id"], "snapshot-020");
    assert_eq!(
        audit["latest_pointer_uri"],
        "s3://bolt-ra-artifacts/prod/artifact-index/v1/pointers/kind=backtests/latest.json"
    );
    assert_eq!(
        audit["new_snapshot_content_hash"].as_str().map(str::len),
        Some(64)
    );
    assert!(audit.get("precondition").is_some());
    assert!(audit.get("new_snapshot_sha256").is_none());
    assert!(audit.get("prior_pointer_e_tag").is_none());
    assert_eq!(audit["writer_id"], "backtesting-engine-writer");
    writer
        .append_audit_intent_v1(&root, &outcome.audit_intent)
        .await
        .expect("identical content-addressed audit intent is idempotent");

    let mut conflicting_audit = outcome.audit_intent.clone();
    conflicting_audit.writer_id = "different-writer".to_string();
    let err = writer
        .append_audit_intent_v1(&root, &conflicting_audit)
        .await
        .expect_err("audit intent id must bind the exact CAS tuple");
    assert!(err.to_string().contains("content-address"), "{err}");
}

#[tokio::test]
async fn artifact_index_audit_failure_cannot_advance_latest_pointer() {
    let root = artifact_config().resolve().expect("valid artifact root");
    let store = FailAuditObjectStore::new();
    let writer = ArtifactIndexWriter::new(&store);
    let event = backtest_event(
        root.backtest_run_root(MarketStructureFixture::PerpsSpot, "run-audit-failure"),
        "event-audit-failure",
        "run-audit-failure",
    );
    writer
        .commit_event(&root, commit_plan(event, &["snapshot-audit-failure"]))
        .await
        .expect_err("audit failure must abort before pointer CAS");
    assert!(
        writer
            .read_latest_pointer(&root, ArtifactKind::Backtests)
            .await
            .expect("pointer read")
            .is_none(),
        "latest pointer must remain absent when audit prewrite fails"
    );
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
        .commit_event(&root, commit_plan(committed, &["snapshot-040"]))
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
        .commit_event(&root, commit_plan(parent, &["snapshot-catalog-060"]))
        .await
        .expect("parent commits without object-store listing");

    let child = backtest_event(
        root.backtest_run_root(MarketStructureFixture::BinaryOption, "run-060"),
        "event-061",
        "run-060",
    );
    writer
        .commit_event(&root, commit_plan(child, &["snapshot-backtest-060"]))
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
            commit_plan(declared_parent, &["snapshot-catalog-050"]),
        )
        .await
        .expect("declared parent commits");

    let child = backtest_event(
        root.backtest_run_root(MarketStructureFixture::BinaryOption, "run-050"),
        "event-051",
        "run-050",
    );
    writer
        .commit_event(&root, commit_plan(child, &["snapshot-backtest-050"]))
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
            commit_plan(independent_latest, &["snapshot-catalog-052"]),
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
            commit_plan_with_writer(event, &["snapshot-030"], "research-analytics-writer"),
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
