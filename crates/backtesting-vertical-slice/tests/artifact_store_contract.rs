use object_store::{ObjectStoreExt, memory::InMemory};

use backtesting_vertical_slice::{
    artifact_store::{
        ArtifactIndexCommitPlan, ArtifactIndexCommitState, ArtifactIndexEvent,
        ArtifactIndexPointer, ArtifactIndexSnapshot, ArtifactIndexSnapshotRow,
        ArtifactIndexWriteAuthority, ArtifactIndexWriter, ArtifactKind, ArtifactLifecycleState,
        ArtifactLineageRef, ArtifactStorageProfile, ArtifactStoreConfig, CatalogDispatchConfig,
        CatalogProjectionBinding, CreateOnlyArtifactWriter, StoredArtifactIndexPointer,
    },
    run_manifest::MarketStructureFixture,
};

fn artifact_config() -> ArtifactStoreConfig {
    toml::from_str(artifact_config_toml()).expect("artifact config parses")
}

fn artifact_config_toml() -> &'static str {
    r#"
artifact_root = "s3://bolt-ra-artifacts/prod"

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
        event_id: event_id.to_string(),
        artifact_kind: ArtifactKind::Backtests,
        artifact_id: artifact_id.to_string(),
        artifact_uri: format!("{root_uri}result.json"),
        manifest_uri: format!("{root_uri}manifest.json"),
        producer_project: "backtesting-engine".to_string(),
        content_sha256: sha256('a'),
        parent_lineage: vec![ArtifactLineageRef {
            artifact_kind: ArtifactKind::NtCatalog,
            artifact_id: "projection-001".to_string(),
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
