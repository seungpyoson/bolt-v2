use backtesting_vertical_slice::artifact_index::{
    ArtifactIndexLineageRef, ArtifactIndexRecord, ArtifactKind, ArtifactLifecycleConfig,
    CommitState, LifecycleState, StorageProfile, WriteAuthority,
};

fn sha256_a() -> String {
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string()
}

fn sha256_b() -> String {
    "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_string()
}

#[test]
fn backtest_index_record_generates_paths_under_single_artifact_root() {
    let root = "s3://example-bucket/nt-research-analytics";
    let record = ArtifactIndexRecord::new_staged(
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
    .expect("index record");

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
    let record = ArtifactIndexRecord::new_staged(
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
    .expect("index record");

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
