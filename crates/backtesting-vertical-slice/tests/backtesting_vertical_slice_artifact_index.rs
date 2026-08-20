use backtesting_vertical_slice::artifact_index::{
    ArtifactIndexEventObject, ArtifactIndexEventWriteDecision, ArtifactIndexLatestPointer,
    ArtifactIndexLineageRef, ArtifactIndexObservedPointer, ArtifactIndexPointerPrecondition,
    ArtifactIndexRecord, ArtifactIndexSnapshotManifest, ArtifactKind, ArtifactLifecycleConfig,
    CommitState, LifecycleState, ResearchAnalyticsSubfamily, StorageProfile, WriteAuthority,
    plan_index_event_create, plan_latest_pointer_update, resolve_committed_snapshot,
    resolve_lineage_parent,
};

fn sha256_a() -> String {
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string()
}

fn sha256_b() -> String {
    "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_string()
}

fn sha256_c() -> String {
    "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc".to_string()
}

fn backtest_record(root: &str) -> ArtifactIndexRecord {
    ArtifactIndexRecord::new_staged(
        root,
        ArtifactKind::Backtests,
        "backtest-run-123",
        "backtesting-engine",
        "s3://example-bucket/nt-research-analytics/backtests/backtest-run-123/result-contract.json",
        &sha256_a(),
        vec![ArtifactIndexLineageRef::new(
            "source-proof-123",
            Some(1),
            sha256_b(),
        )],
    )
    .expect("index record")
}

fn committed_backtest_record(
    root: &str,
    snapshot_id: &str,
    snapshot_uri: &str,
) -> ArtifactIndexRecord {
    backtest_record(root)
        .committed_for_snapshot(snapshot_id, snapshot_uri)
        .expect("committed record")
}

fn source_proof_record(root: &str) -> ArtifactIndexRecord {
    ArtifactIndexRecord::new_staged(
        root,
        ArtifactKind::SourceProofs,
        "source-proof-123",
        "source-proof",
        "s3://example-bucket/nt-research-analytics/source-proofs/source-proof-123/report.json",
        &sha256_b(),
        vec![ArtifactIndexLineageRef::new(
            "raw-object-123",
            None,
            sha256_c(),
        )],
    )
    .expect("source-proof record")
}

#[test]
fn backtest_index_record_generates_paths_under_single_artifact_root() {
    let root = "s3://example-bucket/nt-research-analytics";
    let record = backtest_record(root);

    assert_eq!(record.artifact_kind, ArtifactKind::Backtests);
    assert_eq!(record.commit_state, CommitState::Staged);
    assert_eq!(record.write_authority, WriteAuthority::ProducerOwned);
    assert_eq!(record.lifecycle_state, LifecycleState::Active);
    assert_eq!(
        record.event_uri,
        format!(
            "{root}/artifact-index/v1/events/kind=backtests/artifact_id=backtest-run-123/hash={}.json",
            sha256_a()
        )
    );
    assert_eq!(
        record.latest_pointer_uri,
        format!("{root}/artifact-index/v1/pointers/kind=backtests/latest.json")
    );
    assert!(
        record.snapshot_uri.is_none(),
        "staged records must not claim committed snapshot discovery"
    );
}

#[test]
fn artifact_index_rejects_missing_lineage_and_non_sha256_hashes() {
    let root = "s3://example-bucket/nt-research-analytics";
    let missing_lineage = ArtifactIndexRecord::new_staged(
        root,
        ArtifactKind::Backtests,
        "backtest-run-123",
        "backtesting-engine",
        "s3://example-bucket/nt-research-analytics/backtests/backtest-run-123/result-contract.json",
        &sha256_a(),
        vec![],
    )
    .unwrap_err();
    assert!(
        missing_lineage.to_string().contains("lineage"),
        "{missing_lineage}"
    );

    let invalid_hash = ArtifactIndexRecord::new_staged(
        root,
        ArtifactKind::Backtests,
        "backtest-run-123",
        "backtesting-engine",
        "s3://example-bucket/nt-research-analytics/backtests/backtest-run-123/result-contract.json",
        "etag-123",
        vec![ArtifactIndexLineageRef::new(
            "source-proof-123",
            Some(1),
            sha256_b(),
        )],
    )
    .unwrap_err();
    assert!(
        invalid_hash.to_string().contains("sha256"),
        "{invalid_hash}"
    );
}

#[test]
fn artifact_index_rejects_consumer_mutation_of_producer_records() {
    let root = "s3://example-bucket/nt-research-analytics";
    let record = backtest_record(root);

    record
        .validate_write_authority("backtesting-engine")
        .expect("producer can write its own event");
    let err = record
        .validate_write_authority("research-analytics")
        .unwrap_err();
    assert!(err.to_string().contains("read-only consumer"), "{err}");
}

#[test]
fn lifecycle_config_rejects_default_delete_or_expiration_rules() {
    let mut delete_config = ArtifactLifecycleConfig::retain_forever(86_400);
    delete_config.default_delete_after_seconds = Some(86_400);
    let err = delete_config.validate().unwrap_err();
    assert!(err.to_string().contains("delete"), "{err}");

    let mut expiration_config = ArtifactLifecycleConfig::retain_forever(86_400);
    expiration_config.default_expire_after_seconds = Some(86_400);
    let err = expiration_config.validate().unwrap_err();
    assert!(err.to_string().contains("expiration"), "{err}");
}

#[test]
fn lifecycle_state_follows_configured_quiet_window() {
    let config = ArtifactLifecycleConfig::retain_forever(86_400);

    assert_eq!(
        config.lifecycle_state_at(1_000, 1_000 + 86_399).unwrap(),
        LifecycleState::Active
    );
    assert_eq!(
        config.lifecycle_state_at(1_000, 1_000 + 86_400).unwrap(),
        LifecycleState::Inactive
    );
}

#[test]
fn lifecycle_config_requires_all_storage_profiles() {
    let mut config = ArtifactLifecycleConfig::retain_forever(86_400);
    config.storage_profiles = vec![StorageProfile::Active, StorageProfile::Archive];

    let err = config.validate().unwrap_err();

    assert!(err.to_string().contains("deep_archive"), "{err}");
}

#[test]
fn experiment_contracts_use_one_research_analytics_subfamily() {
    let root = "s3://example-bucket/nt-research-analytics";
    let record = ArtifactIndexRecord::new_research_analytics_staged(
        root,
        ResearchAnalyticsSubfamily::ExperimentContracts,
        "experiment-synthetic-version",
        "bolt-v2",
        format!(
            "{root}/research-analytics/v1/experiment-contracts/synthetic/version/envelope.json"
        ),
        &sha256_a(),
        vec![ArtifactIndexLineageRef::new(
            "governance-authority",
            Some(1),
            sha256_b(),
        )],
    )
    .expect("experiment-contract index record");
    assert_eq!(
        record.artifact_subfamily.as_deref(),
        Some("experiment-contracts")
    );
    assert_eq!(record.lifecycle_state, LifecycleState::Active);

    let error = ArtifactIndexRecord::new_research_analytics_staged(
        root,
        ResearchAnalyticsSubfamily::ExperimentContracts,
        "experiment-synthetic-version",
        "bolt-v2",
        format!("{root}/research-analytics/v1/experiment-results/result.json"),
        &sha256_a(),
        vec![ArtifactIndexLineageRef::new(
            "governance-authority",
            Some(1),
            sha256_b(),
        )],
    )
    .unwrap_err();
    assert!(error.to_string().contains("experiment-contracts"));
}

#[test]
fn committed_snapshot_resolution_rejects_hash_invalid_latest_pointer() {
    let root = "s3://example-bucket/nt-research-analytics";
    let snapshot_id = "snapshot-2026-06-06";
    let snapshot_uri = format!(
        "{root}/artifact-index/v1/snapshots/kind=backtests/snapshot_id={snapshot_id}/manifest.json"
    );
    let snapshot = ArtifactIndexSnapshotManifest::new(
        root,
        ArtifactKind::Backtests,
        snapshot_id,
        &sha256_c(),
        vec![committed_backtest_record(root, snapshot_id, &snapshot_uri)],
    )
    .expect("snapshot");
    let mut pointer = ArtifactIndexLatestPointer::from_snapshot(root, &snapshot).expect("pointer");
    pointer.snapshot_content_hash = sha256_b();

    let err = resolve_committed_snapshot(&pointer, &snapshot).unwrap_err();

    assert!(err.to_string().contains("snapshot hash"), "{err}");
}

#[test]
fn committed_snapshot_resolution_rejects_stale_latest_pointer() {
    let root = "s3://example-bucket/nt-research-analytics";
    let snapshot_id = "snapshot-2026-06-06";
    let snapshot_uri = format!(
        "{root}/artifact-index/v1/snapshots/kind=backtests/snapshot_id={snapshot_id}/manifest.json"
    );
    let snapshot = ArtifactIndexSnapshotManifest::new(
        root,
        ArtifactKind::Backtests,
        snapshot_id,
        &sha256_c(),
        vec![committed_backtest_record(root, snapshot_id, &snapshot_uri)],
    )
    .expect("snapshot");
    let mut pointer = ArtifactIndexLatestPointer::from_snapshot(root, &snapshot).expect("pointer");
    pointer.snapshot_id = "older-snapshot".to_string();

    let err = resolve_committed_snapshot(&pointer, &snapshot).unwrap_err();

    assert!(err.to_string().contains("stale"), "{err}");
}

#[test]
fn committed_snapshot_resolution_requires_hot_index_metadata_active() {
    let root = "s3://example-bucket/nt-research-analytics";
    let snapshot_id = "snapshot-2026-06-06";
    let snapshot_uri = format!(
        "{root}/artifact-index/v1/snapshots/kind=backtests/snapshot_id={snapshot_id}/manifest.json"
    );
    let mut snapshot = ArtifactIndexSnapshotManifest::new(
        root,
        ArtifactKind::Backtests,
        snapshot_id,
        &sha256_c(),
        vec![committed_backtest_record(root, snapshot_id, &snapshot_uri)],
    )
    .expect("snapshot");
    let pointer = ArtifactIndexLatestPointer::from_snapshot(root, &snapshot).expect("pointer");
    snapshot.lifecycle_state = LifecycleState::Inactive;

    let err = resolve_committed_snapshot(&pointer, &snapshot).unwrap_err();

    assert!(err.to_string().contains("active storage"), "{err}");
}

#[test]
fn committed_snapshot_rejects_staged_or_orphan_records_as_discovery_truth() {
    let root = "s3://example-bucket/nt-research-analytics";
    let snapshot_id = "snapshot-2026-06-06";

    let staged_err = ArtifactIndexSnapshotManifest::new(
        root,
        ArtifactKind::Backtests,
        snapshot_id,
        &sha256_c(),
        vec![backtest_record(root)],
    )
    .unwrap_err();
    assert!(staged_err.to_string().contains("committed"), "{staged_err}");

    let snapshot_uri = format!(
        "{root}/artifact-index/v1/snapshots/kind=backtests/snapshot_id={snapshot_id}/manifest.json"
    );
    let mut orphan = committed_backtest_record(root, snapshot_id, &snapshot_uri);
    orphan.commit_state = CommitState::Orphan;
    let orphan_err = ArtifactIndexSnapshotManifest::new(
        root,
        ArtifactKind::Backtests,
        snapshot_id,
        &sha256_c(),
        vec![orphan],
    )
    .unwrap_err();
    assert!(orphan_err.to_string().contains("committed"), "{orphan_err}");
}

#[test]
fn latest_pointer_update_plan_uses_create_or_etag_preconditions() {
    let root = "s3://example-bucket/nt-research-analytics";
    let first_snapshot_id = "snapshot-2026-06-06-a";
    let first_snapshot_uri = format!(
        "{root}/artifact-index/v1/snapshots/kind=backtests/snapshot_id={first_snapshot_id}/manifest.json"
    );
    let first_snapshot = ArtifactIndexSnapshotManifest::new(
        root,
        ArtifactKind::Backtests,
        first_snapshot_id,
        &sha256_c(),
        vec![committed_backtest_record(
            root,
            first_snapshot_id,
            &first_snapshot_uri,
        )],
    )
    .expect("first snapshot");
    let first_pointer =
        ArtifactIndexLatestPointer::from_snapshot(root, &first_snapshot).expect("first pointer");
    let create_plan = plan_latest_pointer_update(root, "backtesting-engine", None, &first_pointer)
        .expect("create plan");
    assert_eq!(
        create_plan.precondition,
        ArtifactIndexPointerPrecondition::IfNoneMatchAny
    );

    let observed_first =
        ArtifactIndexObservedPointer::new(first_pointer, "etag-before").expect("observed pointer");
    let second_snapshot_id = "snapshot-2026-06-06-b";
    let second_snapshot_uri = format!(
        "{root}/artifact-index/v1/snapshots/kind=backtests/snapshot_id={second_snapshot_id}/manifest.json"
    );
    let second_snapshot = ArtifactIndexSnapshotManifest::new(
        root,
        ArtifactKind::Backtests,
        second_snapshot_id,
        &sha256_b(),
        vec![committed_backtest_record(
            root,
            second_snapshot_id,
            &second_snapshot_uri,
        )],
    )
    .expect("second snapshot");
    let second_pointer =
        ArtifactIndexLatestPointer::from_snapshot(root, &second_snapshot).expect("second pointer");
    let update_plan = plan_latest_pointer_update(
        root,
        "backtesting-engine",
        Some(&observed_first),
        &second_pointer,
    )
    .expect("update plan");

    assert_eq!(
        update_plan.precondition,
        ArtifactIndexPointerPrecondition::IfMatch {
            etag: "etag-before".to_string()
        }
    );
    assert!(update_plan.requires_retry_rebase_after_conditional_failure());
    assert_eq!(update_plan.audit_intent.audit_intent_id.len(), 64);
    assert!(
        update_plan
            .audit_intent
            .audit_intent_id
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    );
    assert_eq!(
        update_plan.audit_intent_uri,
        format!(
            "{root}/artifact-index/v1/audit/intents/v1/kind=backtests/{}.json",
            update_plan.audit_intent.audit_intent_id
        )
    );
    let audit_wire = serde_json::to_value(&update_plan.audit_intent).expect("audit intent wire");
    assert!(audit_wire.get("new_snapshot_content_hash").is_some());
    assert!(audit_wire.get("precondition").is_some());
    assert!(audit_wire.get("new_snapshot_sha256").is_none());
    assert!(audit_wire.get("prior_pointer_e_tag").is_none());
    let mut mutated = update_plan.audit_intent.clone();
    mutated.writer_id = "different-writer".to_string();
    assert!(mutated.validate().is_err());
    let mut inconsistent = update_plan.audit_intent.clone();
    inconsistent.prior_snapshot_id = None;
    let error = inconsistent.validate().unwrap_err();
    assert!(error.to_string().contains("precondition disagree"));
}

#[test]
fn cross_kind_parent_resolution_uses_manifest_lineage_hashes() {
    let root = "s3://example-bucket/nt-research-analytics";
    let parent = source_proof_record(root);
    let child = backtest_record(root);

    let resolved = resolve_lineage_parent(&child, &parent).expect("lineage parent");

    assert_eq!(resolved.artifact_id, parent.artifact_id);
    assert_eq!(resolved.content_hash, parent.content_hash);
}

#[test]
fn cross_kind_parent_resolution_rejects_independent_latest_parent_hash() {
    let root = "s3://example-bucket/nt-research-analytics";
    let mut stale_parent = source_proof_record(root);
    stale_parent.content_hash = sha256_c();
    let child = backtest_record(root);

    let err = resolve_lineage_parent(&child, &stale_parent).unwrap_err();

    assert!(err.to_string().contains("manifest lineage"), "{err}");
}

#[test]
fn immutable_event_create_is_idempotent_for_same_payload() {
    let root = "s3://example-bucket/nt-research-analytics";
    let record = backtest_record(root);
    let event = ArtifactIndexEventObject::from_record(&record).expect("event");

    let decision =
        plan_index_event_create("backtesting-engine", &event, Some(&event)).expect("decision");

    assert_eq!(
        decision,
        ArtifactIndexEventWriteDecision::AlreadyExistsSamePayload
    );
}

#[test]
fn immutable_event_create_rejects_different_payload_at_same_uri() {
    let root = "s3://example-bucket/nt-research-analytics";
    let record = backtest_record(root);
    let event = ArtifactIndexEventObject::from_record(&record).expect("event");
    let mut mutated_existing = event.clone();
    mutated_existing.payload_hash = sha256_c();

    let err =
        plan_index_event_create("backtesting-engine", &event, Some(&mutated_existing)).unwrap_err();

    assert!(err.to_string().contains("overwrite"), "{err}");
}

#[test]
fn research_analytics_artifacts_use_typed_subfamilies_and_one_kind_pointer() {
    let root = "s3://example-bucket/nt-research-analytics";
    let record = ArtifactIndexRecord::new_research_analytics_staged(
        root,
        ResearchAnalyticsSubfamily::ExperimentResults,
        "experiment-result-123",
        "research-analytics",
        "s3://example-bucket/nt-research-analytics/research-analytics/v1/experiment-results/experiment-123/manifest.json",
        &sha256_a(),
        vec![ArtifactIndexLineageRef::new(
            "backtest-run-123",
            Some(1),
            sha256_b(),
        )],
    )
    .expect("research analytics record");

    assert_eq!(record.artifact_kind, ArtifactKind::ResearchAnalytics);
    assert_eq!(
        record.artifact_subfamily.as_deref(),
        Some("experiment-results")
    );
    assert_eq!(
        record.event_uri,
        format!(
            "{root}/artifact-index/v1/events/kind=research_analytics/artifact_id=experiment-result-123/hash={}.json",
            sha256_a()
        )
    );
    assert_eq!(
        record.latest_pointer_uri,
        format!("{root}/artifact-index/v1/pointers/kind=research_analytics/latest.json")
    );
    assert!(
        !record.latest_pointer_uri.contains("experiment-results"),
        "RA subfamilies must not get separate latest pointers"
    );
}

#[test]
fn research_analytics_records_require_matching_subfamily_prefix() {
    let root = "s3://example-bucket/nt-research-analytics";
    let err = ArtifactIndexRecord::new_staged(
        root,
        ArtifactKind::ResearchAnalytics,
        "experiment-result-123",
        "research-analytics",
        "s3://example-bucket/nt-research-analytics/research-analytics/v1/experiment-results/experiment-123/manifest.json",
        &sha256_a(),
        vec![ArtifactIndexLineageRef::new(
            "backtest-run-123",
            Some(1),
            sha256_b(),
        )],
    )
    .unwrap_err();
    assert!(err.to_string().contains("subfamily"), "{err}");

    let err = ArtifactIndexRecord::new_research_analytics_staged(
        root,
        ResearchAnalyticsSubfamily::ExperimentResults,
        "experiment-result-123",
        "research-analytics",
        "s3://example-bucket/nt-research-analytics/research-analytics/v1/datasets/experiment-123/manifest.json",
        &sha256_a(),
        vec![ArtifactIndexLineageRef::new(
            "backtest-run-123",
            Some(1),
            sha256_b(),
        )],
    )
    .unwrap_err();
    assert!(err.to_string().contains("experiment-results"), "{err}");
}
