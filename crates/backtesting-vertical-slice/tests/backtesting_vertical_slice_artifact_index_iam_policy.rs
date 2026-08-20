use backtesting_vertical_slice::{
    artifact_index::ArtifactKind,
    artifact_index_iam_policy::{
        ArtifactIndexProducerIamProvisioningPlanSpec, artifact_index_producer_iam_policy,
        artifact_index_producer_iam_provisioning_plan,
    },
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

#[test]
fn producer_iam_policy_uses_canonical_audit_kind_labels_for_every_kind() {
    let cases = [
        (ArtifactKind::Raw, "raw", "raw"),
        (ArtifactKind::NtCatalog, "nt_catalog", "nt-catalog"),
        (ArtifactKind::SourceProofs, "source_proofs", "source-proofs"),
        (ArtifactKind::Backtests, "backtests", "backtests"),
        (
            ArtifactKind::ArtifactIndex,
            "artifact_index",
            "artifact-index",
        ),
        (
            ArtifactKind::ResearchAnalytics,
            "research_analytics",
            "research-analytics",
        ),
    ];

    for (kind, existing_index_label, canonical_audit_label) in cases {
        let policy =
            artifact_index_producer_iam_policy("s3://example-bucket/example-root", kind, &[])
                .expect("policy");
        let resources = &policy.statements[0].resources;
        assert!(resources.iter().any(|resource| resource.ends_with(&format!(
            "artifact-index/v1/events/kind={existing_index_label}/*"
        ))));
        assert!(resources.iter().any(|resource| resource.ends_with(&format!(
            "artifact-index/v1/audit/intents/v1/kind={canonical_audit_label}/*"
        ))));
    }
}

#[test]
fn producer_iam_provisioning_plan_binds_policy_ssm_paths_and_denied_probe_kinds() {
    let plan = artifact_index_producer_iam_provisioning_plan(
        ArtifactIndexProducerIamProvisioningPlanSpec {
            artifact_root: "s3://example-bucket/example-root".to_string(),
            artifact_kind: ArtifactKind::Backtests,
            proof_artifact_roots: vec![
                "s3://example-bucket/example-root/artifact-index/proofs/proof-root".to_string(),
            ],
            ssm_parameter_prefix: "/example/artifact-index/producers".to_string(),
            denied_artifact_kinds: vec![ArtifactKind::ResearchAnalytics],
        },
    )
    .expect("provisioning plan");

    assert_eq!(plan.artifact_kind, ArtifactKind::Backtests);
    assert_eq!(
        plan.ssm_parameter_paths.access_key_id,
        "/example/artifact-index/producers/backtests/access-key-id"
    );
    assert_eq!(
        plan.ssm_parameter_paths.secret_access_key,
        "/example/artifact-index/producers/backtests/secret-access-key"
    );
    assert_eq!(plan.ssm_parameter_paths.session_token, None);
    assert_eq!(
        plan.proof_denied_artifact_kinds,
        vec![ArtifactKind::ResearchAnalytics]
    );
    assert_eq!(plan.expected_denied_write_attempts, 4);

    let resources: Vec<&str> = plan
        .policy
        .statements
        .iter()
        .flat_map(|statement| statement.resources.iter().map(String::as_str))
        .collect();
    assert!(
        resources
            .iter()
            .any(|resource| { resource.ends_with("artifact-index/v1/events/kind=backtests/*") })
    );
    assert!(
        resources
            .iter()
            .any(|resource| resource
                .ends_with("artifact-index/v1/audit/intents/v1/kind=backtests/*"))
    );
    assert!(
        resources
            .iter()
            .all(|resource| !resource.contains("artifact-index/v1/audit/epochs/"))
    );
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
}

#[test]
fn producer_iam_provisioning_plan_rejects_unscoped_prefix_and_self_denial() {
    let invalid_prefix = artifact_index_producer_iam_provisioning_plan(
        ArtifactIndexProducerIamProvisioningPlanSpec {
            artifact_root: "s3://example-bucket/example-root".to_string(),
            artifact_kind: ArtifactKind::Backtests,
            proof_artifact_roots: Vec::new(),
            ssm_parameter_prefix: "example/artifact-index/producers".to_string(),
            denied_artifact_kinds: vec![ArtifactKind::ResearchAnalytics],
        },
    )
    .expect_err("relative SSM prefix must be rejected");
    assert!(invalid_prefix.to_string().contains("absolute SSM"));

    let self_denial = artifact_index_producer_iam_provisioning_plan(
        ArtifactIndexProducerIamProvisioningPlanSpec {
            artifact_root: "s3://example-bucket/example-root".to_string(),
            artifact_kind: ArtifactKind::Backtests,
            proof_artifact_roots: Vec::new(),
            ssm_parameter_prefix: "/example/artifact-index/producers".to_string(),
            denied_artifact_kinds: vec![ArtifactKind::Backtests],
        },
    )
    .expect_err("producer kind cannot be denied by its own proof");
    assert!(self_denial.to_string().contains("denied_artifact_kinds"));

    let broad_artifact_store_prefix = artifact_index_producer_iam_provisioning_plan(
        ArtifactIndexProducerIamProvisioningPlanSpec {
            artifact_root: "s3://example-bucket/example-root".to_string(),
            artifact_kind: ArtifactKind::Backtests,
            proof_artifact_roots: Vec::new(),
            ssm_parameter_prefix: "/example/artifact-store/s3".to_string(),
            denied_artifact_kinds: vec![ArtifactKind::ResearchAnalytics],
        },
    )
    .expect_err("broad artifact-store SSM prefix cannot back producer IAM");
    assert!(
        broad_artifact_store_prefix
            .to_string()
            .contains("artifact-index/producers")
    );
}
