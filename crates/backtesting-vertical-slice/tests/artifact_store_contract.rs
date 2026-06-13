use futures_util::stream::BoxStream;
use object_store::{
    CopyOptions, GetOptions, GetResult, ListResult, MultipartUpload, ObjectMeta, ObjectStore,
    ObjectStoreExt, PutMultipartOptions, PutOptions, PutPayload, PutResult,
    Result as ObjectStoreResult, memory::InMemory, path::Path as ObjectPath,
};
use std::{fmt, fs};

use backtesting_vertical_slice::{
    artifact_store::{
        ArtifactIndexCommitPlan, ArtifactIndexCommitState, ArtifactIndexEvent,
        ArtifactIndexPointer, ArtifactIndexSnapshot, ArtifactIndexSnapshotRow,
        ArtifactIndexWriteAuthority, ArtifactIndexWriter, ArtifactKind, ArtifactLifecycleState,
        ArtifactLineageRef, ArtifactStorageProfile, ArtifactStoreConfig, CatalogDispatchConfig,
        CatalogProjectionBinding, CreateOnlyArtifactWriter, StoredArtifactIndexPointer,
        persist_catalog_projection_for_source_binding,
    },
    run_manifest::MarketStructureFixture,
};

fn artifact_config() -> ArtifactStoreConfig {
    toml::from_str(artifact_config_toml()).expect("artifact config parses")
}

fn artifact_config_toml() -> &'static str {
    r#"
artifact_root = "s3://bolt-ra-artifacts/prod"

[s3]
region = "us-east-1"
conditional_put = "etag"
copy_if_not_exists = "multipart"

[create_only_probe]
prefix = ".writer-probe"
object_name = "sentinel"

[subpaths]
raw = "raw"
nt_catalog = "nt-catalog"
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
    let _store = config
        .build_s3_object_store()
        .expect("S3 object store builder accepts required capability config");
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
fn rejects_local_or_non_s3_canonical_artifact_roots() {
    let mut config = artifact_config();
    config.artifact_root = "/tmp/not-canonical".to_string();
    assert!(config.resolve().is_err());

    config.artifact_root = "file:///tmp/not-canonical".to_string();
    assert!(config.resolve().is_err());
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
        .catalog_root_for("binary-official", &root)
        .expect("binary binding dispatches");
    let perps = dispatch
        .catalog_root_for("perps-official", &root)
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
    assert!(dispatch.catalog_root_for("missing-binding", &root).is_err());
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
    assert!(transcript.first_create_succeeded);
    assert!(transcript.duplicate_create_rejected);
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
        temp.path(),
    )
    .await
    .expect("catalog persisted");

    assert_eq!(
        persisted.catalog_root_uri,
        "s3://bolt-ra-artifacts/prod/nt-catalog/v1/projection=projection-run-123/"
    );
    assert_eq!(persisted.objects.len(), 2);
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
    persist_catalog_projection_for_source_binding(
        &store,
        &root,
        &dispatch,
        "binary-official",
        temp.path(),
    )
    .await
    .expect("first catalog persist");
    fs::write(&catalog_file, b"second").expect("second catalog data");

    let err = persist_catalog_projection_for_source_binding(
        &store,
        &root,
        &dispatch,
        "binary-official",
        temp.path(),
    )
    .await
    .expect_err("duplicate projection bytes must be rejected");

    assert!(err.to_string().contains("differs"), "{err}");
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
    assert!(err.to_string().contains("already exists"), "{err}");

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
    conflicting_audit.writer_id = "different-writer".to_string();
    let err = writer
        .append_audit_epoch(&root, &conflicting_audit)
        .await
        .expect_err("audit epoch create-only write rejects different payload");
    assert!(err.to_string().contains("different payload"), "{err}");
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
    assert!(err.to_string().contains("declared lineage"), "{err}");
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
