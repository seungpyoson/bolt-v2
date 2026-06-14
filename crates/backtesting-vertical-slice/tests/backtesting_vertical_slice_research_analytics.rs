use backtesting_vertical_slice::{
    research_analytics::{
        BacktestEvidenceRef, PostApprovalAction, PromotionPackage, PromotionPackageError,
        PromotionStatus, SourceProofEvidenceRef,
    },
    source_proof::SourceProofFidelityClass,
};

fn source_ref(accepted: bool) -> SourceProofEvidenceRef {
    SourceProofEvidenceRef {
        source_proof_id: "source-proof-example-trades".to_string(),
        source_proof_version: Some(1),
        source_proof_report_uri:
            "s3://example-bucket/nt-research-analytics/source-proofs/example/report.json"
                .to_string(),
        source_proof_report_hash:
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
        fidelity_class: SourceProofFidelityClass::TradeReplay,
        accepted,
    }
}

fn backtest_ref(objective: bool) -> BacktestEvidenceRef {
    BacktestEvidenceRef {
        result_contract_id: "backtest-result-example".to_string(),
        result_contract_uri:
            "s3://example-bucket/nt-research-analytics/backtests/example/result-contract.json"
                .to_string(),
        result_contract_hash: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
            .to_string(),
        objective,
    }
}

fn valid_package() -> PromotionPackage {
    PromotionPackage {
        package_version: 1,
        artifact_root: "s3://example-bucket/nt-research-analytics".to_string(),
        artifact_uri: "s3://example-bucket/nt-research-analytics/research-analytics/v1/promotion-packages/package-123/promotion-package.toml"
            .to_string(),
        status: PromotionStatus::ApprovedForConfig,
        source_proof_refs: vec![source_ref(true)],
        backtest_result_refs: vec![backtest_ref(true)],
        preserved_claim_limits: vec![
            "trade replay only; no queue-position or order-book-liquidity claims".to_string(),
        ],
        requested_claim_fidelity: SourceProofFidelityClass::TradeReplay,
        typed_config_uri: Some(
            "s3://example-bucket/nt-research-analytics/research-analytics/v1/promotion-packages/package-123/runtime-config.toml"
                .to_string(),
        ),
        typed_config_hash: Some(
            "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc".to_string(),
        ),
        dashboard_field_refs: vec!["dashboard:strategy-candidate-summary:v1".to_string()],
        reviewer_policy_refs: vec!["policy:research-review:v1".to_string()],
        non_live_boundary: true,
        notebook_runtime_code_refs: Vec::new(),
        accepts_source_proofs: false,
        mutates_source_proofs: false,
        mutates_backtest_result_contracts: false,
        weakens_forbidden_claims: false,
        post_approval_actions: Vec::new(),
    }
}

#[test]
fn approved_for_config_requires_objective_evidence_and_non_live_boundary() {
    let mut package = valid_package();
    package.source_proof_refs.clear();
    package.backtest_result_refs = vec![backtest_ref(false)];
    package.preserved_claim_limits.clear();
    package.typed_config_uri = None;
    package.typed_config_hash = None;
    package.reviewer_policy_refs.clear();
    package.non_live_boundary = false;

    let err = package
        .validate()
        .expect_err("incomplete approved_for_config package must fail");

    assert!(matches!(
        err,
        PromotionPackageError::ApprovedForConfigMissing { .. }
    ));
    let message = err.to_string();
    assert!(message.contains("accepted source proof refs"), "{message}");
    assert!(
        message.contains("objective backtest result refs"),
        "{message}"
    );
    assert!(message.contains("preserved claim limits"), "{message}");
    assert!(message.contains("typed config uri"), "{message}");
    assert!(message.contains("typed config hash"), "{message}");
    assert!(message.contains("reviewer/policy refs"), "{message}");
    assert!(message.contains("explicit non-live boundary"), "{message}");
}

#[test]
fn promotion_package_rejects_proof_strength_upgrade_and_forbidden_actions() {
    let mut package = valid_package();
    package.requested_claim_fidelity = SourceProofFidelityClass::L2Replay;
    package.accepts_source_proofs = true;
    package.mutates_source_proofs = true;
    package.mutates_backtest_result_contracts = true;
    package.weakens_forbidden_claims = true;
    package.post_approval_actions = vec![
        PostApprovalAction::AutoMerge,
        PostApprovalAction::AutoEnableStrategy,
        PostApprovalAction::ScheduleLiveTrading,
        PostApprovalAction::TouchSsmCredentials,
        PostApprovalAction::MutateProductionRuntimeConfig,
    ];

    let err = package
        .validate()
        .expect_err("proof upgrade and forbidden post-approval actions must fail");

    assert!(matches!(
        err,
        PromotionPackageError::ForbiddenPromotionPackageBehavior { .. }
    ));
    let message = err.to_string();
    assert!(message.contains("proof-strength upgrade"), "{message}");
    assert!(
        message.contains("unauthorized proof acceptance"),
        "{message}"
    );
    assert!(message.contains("source proof mutation"), "{message}");
    assert!(
        message.contains("backtest result contract mutation"),
        "{message}"
    );
    assert!(message.contains("forbidden-claim weakening"), "{message}");
    assert!(message.contains("auto-merge"), "{message}");
    assert!(message.contains("auto-enable strategy"), "{message}");
    assert!(message.contains("schedule live trading"), "{message}");
    assert!(message.contains("touch SSM credentials"), "{message}");
    assert!(
        message.contains("mutate production runtime config"),
        "{message}"
    );
}

#[test]
fn promotion_package_artifacts_must_live_under_ra_promotion_family() {
    let mut package = valid_package();
    package.artifact_uri =
        "s3://example-bucket/nt-research-analytics/backtests/package-123/promotion-package.toml"
            .to_string();

    let err = package
        .validate()
        .expect_err("promotion artifacts outside RA package family must fail");

    assert!(matches!(
        err,
        PromotionPackageError::ArtifactOutsidePromotionFamily { .. }
    ));
}

#[test]
fn promotion_package_rejects_notebook_to_production_direct_promotion() {
    let mut package = valid_package();
    package.notebook_runtime_code_refs = vec![
        "s3://example-bucket/nt-research-analytics/research-analytics/v1/experiment-results/notebook.ipynb"
            .to_string(),
    ];
    package.post_approval_actions = vec![PostApprovalAction::MutateProductionRuntimeConfig];

    let err = package
        .validate()
        .expect_err("notebook runtime code cannot be promoted directly");

    assert!(matches!(
        err,
        PromotionPackageError::ForbiddenPromotionPackageBehavior { .. }
    ));
    let message = err.to_string();
    assert!(message.contains("notebook runtime code"), "{message}");
    assert!(
        message.contains("mutate production runtime config"),
        "{message}"
    );
}

#[test]
fn promotion_package_rejects_cross_family_fidelity_claims() {
    let mut package = valid_package();
    let mut snapshot_ref = source_ref(true);
    snapshot_ref.fidelity_class = SourceProofFidelityClass::SnapshotReplay;
    package.source_proof_refs = vec![snapshot_ref];
    package.requested_claim_fidelity = SourceProofFidelityClass::TradeReplay;

    let err = package
        .validate()
        .expect_err("snapshot replay evidence must not imply trade replay claims");

    assert!(matches!(
        err,
        PromotionPackageError::ForbiddenPromotionPackageBehavior { .. }
    ));
    assert!(
        err.to_string().contains("fidelity-incompatible claim"),
        "{err}"
    );
}

#[test]
fn approved_for_config_accepts_preserved_claim_limited_typed_config_only() {
    valid_package()
        .validate()
        .expect("complete approved_for_config package should validate");
}

#[test]
fn promotion_package_preserves_dashboard_field_refs_as_read_only_metadata() {
    let mut package = valid_package();
    package.status = PromotionStatus::ReadyForReview;
    package.dashboard_field_refs = vec![
        "dashboard:strategy-candidate-summary:v1".to_string(),
        "dashboard:backtest-evidence-link:v1".to_string(),
    ];

    package
        .validate()
        .expect("dashboard refs are metadata, not upstream mutations");
}
