use backtesting_vertical_slice::{
    artifact_index::ArtifactKind, artifact_index_iam_policy::artifact_index_producer_iam_policy,
};

#[test]
fn producer_iam_policy_scopes_index_writes_to_one_configured_kind() {
    let policy = artifact_index_producer_iam_policy(
        "s3://example-bucket/example-root",
        ArtifactKind::Backtests,
        &["s3://example-bucket/example-root/artifact-index/proofs/proof-root"],
    )
    .expect("policy");

    let resources: Vec<&str> = policy
        .statements
        .iter()
        .flat_map(|statement| statement.resources.iter().map(String::as_str))
        .collect();

    assert!(resources.contains(
        &"arn:aws:s3:::example-bucket/example-root/artifact-index/v1/events/kind=backtests/*"
    ));
    assert!(resources.contains(
        &"arn:aws:s3:::example-bucket/example-root/artifact-index/v1/snapshots/kind=backtests/*"
    ));
    assert!(resources.contains(
        &"arn:aws:s3:::example-bucket/example-root/artifact-index/v1/pointers/kind=backtests/latest.json"
    ));
    assert!(resources.contains(
        &"arn:aws:s3:::example-bucket/example-root/artifact-index/proofs/proof-root/artifact-index/v1/events/kind=backtests/*"
    ));
    assert!(
        resources
            .iter()
            .all(|resource| !resource.contains("kind=research_analytics"))
    );
    assert!(
        resources
            .iter()
            .all(|resource| !resource.contains("kind=*"))
    );
    assert!(policy.statements.iter().all(|statement| {
        statement.actions == vec!["s3:GetObject".to_string(), "s3:PutObject".to_string()]
    }));
}
