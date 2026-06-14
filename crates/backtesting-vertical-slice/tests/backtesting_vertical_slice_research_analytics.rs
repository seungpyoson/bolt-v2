use backtesting_vertical_slice::{
    operator::{RESULT_CONTRACT_FILE, RunSpec},
    research_analytics::{
        BacktestEvidenceRef, BacktestSweepPlan, BacktestSweepRun, PostApprovalAction,
        PromotionPackage, PromotionPackageError, PromotionStatus, SourceProofEvidenceRef,
        run_backtest_sweep_with_executor,
    },
    result_contract::{
        BacktestResultContract, NautilusResultPointer, RESULT_CONTRACT_VERSION, ResultArtifactUris,
    },
    source_proof::AcceptanceMode,
    source_proof::SourceProofFidelityClass,
};
use std::{fs, path::Path};
use tempfile::TempDir;

const COMMITTED_RUN_SPEC: &str = include_str!(
    "../../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-run-spec.bnbusdc-2026-03-01.toml"
);

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

fn run_spec(run_id: &str) -> RunSpec {
    let mut spec: RunSpec = toml::from_str(COMMITTED_RUN_SPEC).expect("committed run-spec parses");
    spec.manifest.run_id = run_id.to_string();
    spec
}

fn contract(run_id: &str, result_contract_uri: &str) -> BacktestResultContract {
    BacktestResultContract {
        contract_version: RESULT_CONTRACT_VERSION.to_string(),
        run_id: run_id.to_string(),
        nt_version: "nt-test-rev".to_string(),
        source_proof_id: "source-proof-example-trades".to_string(),
        source_proof_version: 1,
        manifest_hash: "manifest-hash".to_string(),
        acceptance_mode: AcceptanceMode::Manual,
        accepted_by: "research-analytics-test".to_string(),
        accepted_at: "2026-06-14T00:00:00Z".to_string(),
        accepted_object_sha256: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            .to_string(),
        converter_identity: "converter".to_string(),
        converter_version: "converter.v1".to_string(),
        converter_config_hash: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
            .to_string(),
        conversion_manifest_hash:
            "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc".to_string(),
        conversion_checkpoint_hash:
            "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd".to_string(),
        catalog_hash: "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"
            .to_string(),
        catalog_metadata_hash: "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"
            .to_string(),
        event_count_ledger_hash: None,
        selected_asset_ids_hash: None,
        strategy_config_hash: "1111111111111111111111111111111111111111111111111111111111111111"
            .to_string(),
        run_purpose: "normal".to_string(),
        market_structure_fixture: "binary option".to_string(),
        fidelity_class: SourceProofFidelityClass::TradeReplay,
        claim_limits: vec!["trade replay only".to_string()],
        warnings: Vec::new(),
        mechanical_blockers: Vec::new(),
        nt_result: NautilusResultPointer {
            trader_id: "TRADER-001".to_string(),
            machine_id: "machine".to_string(),
            instance_id: "instance".to_string(),
            run_config_id: Some("run-config".to_string()),
            backtest_start: Some(1),
            backtest_end: Some(2),
            elapsed_time_secs: 0.1,
            iterations: 3,
            total_events: 4,
            total_orders: 5,
            total_positions: 6,
        },
        artifact_uris: ResultArtifactUris {
            source_proof_uri: "s3://example-bucket/source-proof.json".to_string(),
            canonical_table_uri: "s3://example-bucket/canonical.parquet".to_string(),
            nt_catalog_uri: "s3://example-bucket/nt-catalog/".to_string(),
            nt_catalog_manifest_uri: None,
            catalog_metadata_uri: "s3://example-bucket/catalog-metadata.json".to_string(),
            result_contract_uri: result_contract_uri.to_string(),
        },
        created_at: "2026-06-14T00:00:01Z".to_string(),
    }
}

fn write_contract(output_dir: &Path, run_id: &str) {
    fs::create_dir_all(output_dir).expect("create run output dir");
    let path = output_dir.join(RESULT_CONTRACT_FILE);
    let artifact = contract(run_id, &path.to_string_lossy());
    fs::write(
        &path,
        serde_json::to_vec_pretty(&artifact).expect("serialize contract"),
    )
    .expect("write result contract");
}

#[test]
fn sweep_orchestration_writes_typed_run_specs_invokes_bte_and_reads_contracts() {
    let temp = TempDir::new().expect("temp dir");
    let first_bytes = b"accepted-object-one".to_vec();
    let second_bytes = b"accepted-object-two".to_vec();
    let plan = BacktestSweepPlan {
        run_spec_dir: temp.path().join("run-spec-output"),
        run_output_dir: temp.path().join("run-output"),
        runs: vec![
            BacktestSweepRun {
                run_spec_file_name: "first-run.toml".to_string(),
                output_dir_name: "first-run".to_string(),
                run_spec: run_spec("ra-sweep-first"),
                accepted_object_bytes: first_bytes.clone(),
            },
            BacktestSweepRun {
                run_spec_file_name: "second-run.toml".to_string(),
                output_dir_name: "second-run".to_string(),
                run_spec: run_spec("ra-sweep-second"),
                accepted_object_bytes: second_bytes.clone(),
            },
        ],
    };
    let mut calls = Vec::new();

    let report = run_backtest_sweep_with_executor(&plan, |spec, object_bytes, output_dir| {
        calls.push((
            spec.manifest.run_id.clone(),
            object_bytes.to_vec(),
            output_dir.to_path_buf(),
        ));
        write_contract(output_dir, &spec.manifest.run_id);
        Ok(())
    })
    .expect("sweep orchestration succeeds");

    assert_eq!(
        calls,
        vec![
            (
                "ra-sweep-first".to_string(),
                first_bytes,
                temp.path().join("run-output").join("first-run"),
            ),
            (
                "ra-sweep-second".to_string(),
                second_bytes,
                temp.path().join("run-output").join("second-run"),
            ),
        ]
    );
    assert_eq!(report.runs.len(), 2);
    assert_eq!(report.runs[0].contract.run_id, "ra-sweep-first");
    assert_eq!(report.runs[1].contract.run_id, "ra-sweep-second");

    let written_toml =
        fs::read_to_string(&report.runs[0].run_spec_path).expect("read written run-spec TOML");
    let reparsed: RunSpec = toml::from_str(&written_toml).expect("written run-spec is typed TOML");
    assert_eq!(reparsed.manifest.run_id, "ra-sweep-first");
    assert_eq!(
        report.runs[0].result_contract_path,
        temp.path()
            .join("run-output")
            .join("first-run")
            .join(RESULT_CONTRACT_FILE)
    );
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
