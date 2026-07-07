use backtesting_vertical_slice::{
    artifact_index::LifecycleState,
    hashing::sha256_hex,
    operator::{RESULT_CONTRACT_FILE, RunSpec},
    research_analytics::{
        ArtifactPointerRef, BacktestEvidenceRef, BacktestRunCatalogList, BacktestSweepPlan,
        BacktestSweepRun, ExperimentResultArtifact, ForbiddenPromotionAction, PromotionConfigRef,
        RaVerdict, RaVerdictKind, ResearchAnalyticsArtifactError, RunPointerIndexRecord,
        RunPointerResult, SourceProofEvidenceRef, build_run_pointer_index_from_catalog,
        run_backtest_sweep_with_executor,
    },
    result_contract::{
        BacktestResultContract, NautilusResultPointer, RESULT_CONTRACT_VERSION, ResultArtifactUris,
    },
    source_proof::AcceptanceMode,
    source_proof::SourceProofFidelityClass,
};
use std::{collections::BTreeMap, fs, path::Path};
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

fn evidence_report_ref() -> ArtifactPointerRef {
    ArtifactPointerRef {
        uri: "s3://example-bucket/nt-research-analytics/research-analytics/v1/experiment-results/experiment-123/report.md"
            .to_string(),
        sha256: "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd".to_string(),
    }
}

fn verdict(kind: RaVerdictKind) -> RaVerdict {
    RaVerdict {
        verdict: kind,
        scope: "lead-lag monthly remeasurement".to_string(),
        source_proof_refs: vec![source_ref(true)],
        backtest_result_refs: vec![backtest_ref(true)],
        evidence_report_refs: vec![evidence_report_ref()],
        requested_claim_fidelity: SourceProofFidelityClass::TradeReplay,
        preserved_claim_limits: vec![
            "trade replay only; no queue-position or order-book-liquidity claims".to_string(),
        ],
        remeasurement_cadence:
            "once after pilot close, monthly thereafter, and after structural market changes"
                .to_string(),
        recorded_at: "2026-06-14T00:00:00Z".to_string(),
        recorded_by: "research-analytics-test".to_string(),
    }
}

fn promotion_config() -> PromotionConfigRef {
    PromotionConfigRef {
        typed_config_uri:
            "s3://example-bucket/nt-research-analytics/research-analytics/v1/experiment-results/experiment-123/runtime-config.toml"
                .to_string(),
        typed_config_hash:
            "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc".to_string(),
        reviewer_policy_refs: vec!["policy:research-review:v1".to_string()],
        non_live_boundary: true,
    }
}

fn valid_experiment_result(kind: RaVerdictKind) -> ExperimentResultArtifact {
    let mut artifact = ExperimentResultArtifact {
        artifact_schema_version: 1,
        artifact_id: "experiment-result-123".to_string(),
        artifact_root: "s3://example-bucket/nt-research-analytics".to_string(),
        artifact_uri: "s3://example-bucket/nt-research-analytics/research-analytics/v1/experiment-results/experiment-123/experiment-result.json"
            .to_string(),
        owner: "research-analytics".to_string(),
        source_refs: vec!["backtest-result-example".to_string()],
        source_hashes: vec![
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_string(),
        ],
        content_hash: "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"
            .to_string(),
        lifecycle_state: LifecycleState::Active,
        verdict: verdict(kind),
        promotion_config: None,
        dashboard_field_refs: vec!["dashboard:strategy-candidate-summary:v1".to_string()],
        notebook_runtime_code_refs: Vec::new(),
        accepts_source_proofs: false,
        mutates_source_proofs: false,
        mutates_backtest_result_contracts: false,
        weakens_forbidden_claims: false,
        post_verdict_actions: Vec::new(),
    };
    artifact.content_hash = artifact.expected_content_hash();
    artifact
}

fn run_spec(run_id: &str, accepted_object_bytes: &[u8]) -> RunSpec {
    let mut spec: RunSpec = toml::from_str(COMMITTED_RUN_SPEC).expect("committed run-spec parses");
    spec.manifest.run_id = run_id.to_string();
    spec.accepted_object.sha256 = sha256_hex(accepted_object_bytes);
    spec
}

fn contract(run_id: &str, result_contract_uri: &str) -> BacktestResultContract {
    BacktestResultContract {
        contract_version: RESULT_CONTRACT_VERSION.to_string(),
        run_id: run_id.to_string(),
        nt_version: "nt-test-rev".to_string(),
        source_proof_id: "source-proof-example-trades".to_string(),
        source_proof_version: 1,
        manifest_hash: "9999999999999999999999999999999999999999999999999999999999999999"
            .to_string(),
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
        execution_model: "nt_backtest_node".to_string(),
        venue_queue_position: Some(false),
        catalog_data_types: vec!["TradeTick".to_string()],
        run_purpose: "normal".to_string(),
        market_structure_fixture: "binary option".to_string(),
        fidelity_class: SourceProofFidelityClass::TradeReplay,
        claim_limits: vec!["trade replay only".to_string()],
        warnings: Vec::new(),
        mechanical_blockers: Vec::new(),
        config_override_report: None,
        run_guard_report: None,
        feed_labels: vec![],
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
            stats_pnls: Default::default(),
            stats_returns: Default::default(),
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

fn contract_for_run_spec(
    spec: &RunSpec,
    object_bytes: &[u8],
    result_contract_uri: &str,
) -> BacktestResultContract {
    let mut artifact = contract(&spec.manifest.run_id, result_contract_uri);
    artifact.nt_version = spec.manifest.resolved_nt_version.clone();
    artifact.source_proof_id = spec.manifest.source_proof_id.clone();
    artifact.source_proof_version = spec.manifest.source_proof_version;
    artifact.manifest_hash = spec.manifest.manifest_hash();
    artifact.accepted_object_sha256 = sha256_hex(object_bytes);
    artifact.converter_config_hash = spec.converter.content_hash().expect("converter hash");
    artifact.strategy_config_hash = spec.manifest.strategy_config_hash.clone();
    artifact
}

fn write_contract(output_dir: &Path, spec: &RunSpec, object_bytes: &[u8]) {
    fs::create_dir_all(output_dir).expect("create run output dir");
    let path = output_dir.join(RESULT_CONTRACT_FILE);
    let result_contract_uri = format!("{}/{}", spec.manifest.output_prefix, RESULT_CONTRACT_FILE);
    let artifact = contract_for_run_spec(spec, object_bytes, &result_contract_uri);
    fs::write(
        &path,
        serde_json::to_vec_pretty(&artifact).expect("serialize contract"),
    )
    .expect("write result contract");
}

struct FakeBacktestRunCatalog {
    run_ids: Vec<String>,
}

impl BacktestRunCatalogList for FakeBacktestRunCatalog {
    fn list_backtest_runs(&self) -> anyhow::Result<Vec<String>> {
        Ok(self.run_ids.clone())
    }
}

fn run_pointer_record(
    artifact_root: &str,
    run_id: &str,
    strategy: &str,
    hash_char: char,
) -> RunPointerIndexRecord {
    let mut params = BTreeMap::new();
    params.insert(
        "strategy".to_string(),
        serde_json::Value::String(strategy.to_string()),
    );
    params.insert("threshold_bps".to_string(), serde_json::json!(12));

    RunPointerIndexRecord {
        run_id: run_id.to_string(),
        params,
        result: RunPointerResult {
            result_contract_uri: format!("{artifact_root}/backtests/{run_id}/result-contract.json"),
            result_contract_hash: hash_char.to_string().repeat(64),
        },
    }
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
                run_spec: run_spec("ra-sweep-first", &first_bytes),
                accepted_object_bytes: first_bytes.clone(),
            },
            BacktestSweepRun {
                run_spec_file_name: "second-run.toml".to_string(),
                output_dir_name: "second-run".to_string(),
                run_spec: run_spec("ra-sweep-second", &second_bytes),
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
        write_contract(output_dir, spec, object_bytes);
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
fn sweep_orchestration_rejects_contract_not_bound_to_run_spec() {
    let temp = TempDir::new().expect("temp dir");
    let accepted_object_bytes = b"accepted-object-one".to_vec();
    let spec = run_spec("ra-sweep-first", &accepted_object_bytes);
    let plan = BacktestSweepPlan {
        run_spec_dir: temp.path().join("run-spec-output"),
        run_output_dir: temp.path().join("run-output"),
        runs: vec![BacktestSweepRun {
            run_spec_file_name: "first-run.toml".to_string(),
            output_dir_name: "first-run".to_string(),
            run_spec: spec.clone(),
            accepted_object_bytes: accepted_object_bytes.clone(),
        }],
    };

    let err = run_backtest_sweep_with_executor(&plan, |spec, object_bytes, output_dir| {
        fs::create_dir_all(output_dir).expect("create output dir");
        let path = output_dir.join(RESULT_CONTRACT_FILE);
        let mut artifact = contract_for_run_spec(spec, object_bytes, &path.to_string_lossy());
        artifact.manifest_hash =
            "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff".to_string();
        fs::write(
            &path,
            serde_json::to_vec_pretty(&artifact).expect("serialize contract"),
        )
        .expect("write mismatched contract");
        Ok(())
    })
    .expect_err("sweep must reject result contracts not bound to the run-spec");

    assert!(err.to_string().contains("manifest_hash"), "{err}");
}

#[test]
fn sweep_orchestration_rejects_existing_run_spec_file_before_executor() {
    let temp = TempDir::new().expect("temp dir");
    let run_spec_dir = temp.path().join("run-spec-output");
    fs::create_dir_all(&run_spec_dir).expect("create run-spec dir");
    fs::write(run_spec_dir.join("first-run.toml"), "stale").expect("write stale run-spec");
    let plan = BacktestSweepPlan {
        run_spec_dir,
        run_output_dir: temp.path().join("run-output"),
        runs: vec![BacktestSweepRun {
            run_spec_file_name: "first-run.toml".to_string(),
            output_dir_name: "first-run".to_string(),
            run_spec: run_spec("ra-sweep-first", b"accepted-object-one"),
            accepted_object_bytes: b"accepted-object-one".to_vec(),
        }],
    };
    let mut calls = 0;

    let err = run_backtest_sweep_with_executor(&plan, |_, _, _| {
        calls += 1;
        Ok(())
    })
    .expect_err("preexisting run-spec path must fail before executor");

    assert_eq!(calls, 0, "executor must not run after stale run-spec");
    assert!(err.to_string().contains("run-spec"), "{err}");
    assert!(err.to_string().contains("already exists"), "{err}");
}

#[test]
fn sweep_orchestration_rejects_existing_output_dir_before_executor() {
    let temp = TempDir::new().expect("temp dir");
    let output_dir = temp.path().join("run-output").join("first-run");
    let accepted_object_bytes = b"accepted-object-one";
    write_contract(
        &output_dir,
        &run_spec("ra-sweep-first", accepted_object_bytes),
        accepted_object_bytes,
    );
    let plan = BacktestSweepPlan {
        run_spec_dir: temp.path().join("run-spec-output"),
        run_output_dir: temp.path().join("run-output"),
        runs: vec![BacktestSweepRun {
            run_spec_file_name: "first-run.toml".to_string(),
            output_dir_name: "first-run".to_string(),
            run_spec: run_spec("ra-sweep-first", accepted_object_bytes),
            accepted_object_bytes: accepted_object_bytes.to_vec(),
        }],
    };
    let mut calls = 0;

    let err = run_backtest_sweep_with_executor(&plan, |_, _, _| {
        calls += 1;
        Ok(())
    })
    .expect_err("preexisting run output dir must fail before executor");

    assert_eq!(calls, 0, "executor must not run against stale output dir");
    assert!(err.to_string().contains("output_dir"), "{err}");
    assert!(err.to_string().contains("already exists"), "{err}");
}

#[test]
fn sweep_orchestration_rejects_duplicate_materialization_paths_before_executor() {
    let temp = TempDir::new().expect("temp dir");
    let plan = BacktestSweepPlan {
        run_spec_dir: temp.path().join("run-spec-output"),
        run_output_dir: temp.path().join("run-output"),
        runs: vec![
            BacktestSweepRun {
                run_spec_file_name: "shared-run.toml".to_string(),
                output_dir_name: "shared-run".to_string(),
                run_spec: run_spec("ra-sweep-first", b"accepted-object-one"),
                accepted_object_bytes: b"accepted-object-one".to_vec(),
            },
            BacktestSweepRun {
                run_spec_file_name: "shared-run.toml".to_string(),
                output_dir_name: "shared-run".to_string(),
                run_spec: run_spec("ra-sweep-second", b"accepted-object-two"),
                accepted_object_bytes: b"accepted-object-two".to_vec(),
            },
        ],
    };
    let mut calls = 0;

    let err = run_backtest_sweep_with_executor(&plan, |_, _, _| {
        calls += 1;
        Ok(())
    })
    .expect_err("duplicate materialization paths must fail before executor");

    assert_eq!(
        calls, 0,
        "executor must not run after duplicate path preflight"
    );
    assert!(err.to_string().contains("duplicate"), "{err}");
}

#[test]
fn run_pointer_index_covers_catalog_runs_with_hash_and_no_lifecycle_or_promotion_state() {
    let artifact_root = "s3://example-bucket/nt-research-analytics";
    let catalog = FakeBacktestRunCatalog {
        run_ids: vec!["ra-run-b".to_string(), "ra-run-a".to_string()],
    };

    let index = build_run_pointer_index_from_catalog(
        &catalog,
        artifact_root,
        vec![
            run_pointer_record(artifact_root, "ra-run-a", "edge-taker", 'a'),
            run_pointer_record(artifact_root, "ra-run-b", "mean-reversion", 'b'),
        ],
    )
    .expect("catalog-listed runs build a run-pointer index");

    index.validate().expect("run-pointer index validates");
    assert_eq!(index.artifact_root, artifact_root);
    assert_eq!(
        index
            .runs
            .iter()
            .map(|entry| entry.run_id.as_str())
            .collect::<Vec<_>>(),
        vec!["ra-run-a", "ra-run-b"]
    );
    assert_eq!(index.content_hash.len(), 64);
    assert!(
        index
            .content_hash
            .chars()
            .all(|ch| ch.is_ascii_digit() || ('a'..='f').contains(&ch))
    );
    assert_eq!(
        index.runs[0].params.get("strategy"),
        Some(&serde_json::Value::String("edge-taker".to_string()))
    );

    let serialized = serde_json::to_value(&index).expect("serialize run-pointer index");
    assert!(serialized.get("lifecycle_state").is_none());
    assert!(serialized.get("promotion_config").is_none());
    for entry in serialized
        .get("runs")
        .and_then(serde_json::Value::as_array)
        .expect("runs serialize as array")
    {
        assert!(entry.get("lifecycle_state").is_none());
        assert!(entry.get("promotion_config").is_none());
    }
}

#[test]
fn run_pointer_index_rejects_records_not_backed_by_one_catalog_root() {
    let artifact_root = "s3://example-bucket/nt-research-analytics";
    let catalog = FakeBacktestRunCatalog {
        run_ids: vec!["ra-run-a".to_string(), "ra-run-b".to_string()],
    };

    let missing_run = build_run_pointer_index_from_catalog(
        &catalog,
        artifact_root,
        vec![run_pointer_record(
            artifact_root,
            "ra-run-a",
            "edge-taker",
            'a',
        )],
    )
    .expect_err("records must cover catalog.list_backtest_runs exactly");
    assert!(
        missing_run
            .to_string()
            .contains("catalog.list_backtest_runs"),
        "{missing_run}"
    );

    let mut foreign_root = run_pointer_record(artifact_root, "ra-run-b", "mean-reversion", 'b');
    foreign_root.result.result_contract_uri =
        "s3://other-bucket/nt-research-analytics/backtests/ra-run-b/result-contract.json"
            .to_string();
    let wrong_root = build_run_pointer_index_from_catalog(
        &catalog,
        artifact_root,
        vec![
            run_pointer_record(artifact_root, "ra-run-a", "edge-taker", 'a'),
            foreign_root,
        ],
    )
    .expect_err("result pointers must share the index artifact_root");
    assert!(
        wrong_root.to_string().contains("artifact_root"),
        "{wrong_root}"
    );
}

#[test]
fn experiment_result_verdict_requires_required_field_set() {
    let artifact = valid_experiment_result(RaVerdictKind::NoGo);

    artifact
        .validate()
        .expect("complete verdict-bearing experiment result should validate");
    assert_eq!(artifact.verdict.scope, "lead-lag monthly remeasurement");
    assert_eq!(
        artifact.verdict.remeasurement_cadence,
        "once after pilot close, monthly thereafter, and after structural market changes"
    );
    assert_eq!(artifact.verdict.source_proof_refs.len(), 1);
    assert_eq!(artifact.verdict.backtest_result_refs.len(), 1);
    assert_eq!(artifact.verdict.evidence_report_refs.len(), 1);
    assert_eq!(artifact.verdict.preserved_claim_limits.len(), 1);

    let mut incomplete = artifact.clone();
    incomplete.verdict.remeasurement_cadence.clear();

    assert!(matches!(
        incomplete
            .validate()
            .expect_err("missing cadence must fail"),
        ResearchAnalyticsArtifactError::EmptyField {
            field: "verdict.remeasurement_cadence"
        }
    ));
}

#[test]
fn promotion_gate_stays_inert_without_go_finding() {
    let artifact = valid_experiment_result(RaVerdictKind::NoGo);

    artifact
        .validate()
        .expect("NO-GO verdict without promotion config stays inert");
    assert!(artifact.promotion_config.is_none());

    let mut illegal = artifact;
    illegal.promotion_config = Some(promotion_config());

    assert!(matches!(
        illegal
            .validate()
            .expect_err("NO-GO verdict must not carry promotion config"),
        ResearchAnalyticsArtifactError::PromotionConfigRequiresGo
    ));
}

#[test]
fn go_finding_can_carry_typed_config_only_on_experiment_result() {
    let mut artifact = valid_experiment_result(RaVerdictKind::Go);
    artifact.promotion_config = Some(promotion_config());
    artifact.content_hash = artifact.expected_content_hash();

    artifact
        .validate()
        .expect("real GO finding may carry typed promotion config field");

    let mut separate_family = artifact.clone();
    separate_family.artifact_uri =
        "s3://example-bucket/nt-research-analytics/research-analytics/v1/promotion-packages/package-123/promotion-package.toml"
            .to_string();

    assert!(matches!(
        separate_family
            .validate()
            .expect_err("experiment result must not live under promotion-packages"),
        ResearchAnalyticsArtifactError::ArtifactOutsideExperimentResults { .. }
    ));

    let mut separate_config = artifact;
    separate_config
        .promotion_config
        .as_mut()
        .expect("promotion config")
        .typed_config_uri =
        "s3://example-bucket/nt-research-analytics/research-analytics/v1/promotion-packages/package-123/runtime-config.toml"
            .to_string();

    assert!(matches!(
        separate_config
            .validate()
            .expect_err("typed config must be a field/URI on experiment-results"),
        ResearchAnalyticsArtifactError::ArtifactOutsideExperimentResults { .. }
    ));
}

#[test]
fn go_promotion_requires_accepted_source_proof_and_objective_backtest_refs() {
    let mut artifact = valid_experiment_result(RaVerdictKind::Go);
    artifact.verdict.source_proof_refs = vec![source_ref(false)];
    artifact.verdict.backtest_result_refs = vec![backtest_ref(true)];

    assert!(matches!(
        artifact
            .validate()
            .expect_err("GO verdict requires accepted source proof evidence"),
        ResearchAnalyticsArtifactError::PromotionConfigRequiresGo
    ));

    let mut artifact = valid_experiment_result(RaVerdictKind::Go);
    artifact.verdict.backtest_result_refs = vec![backtest_ref(false)];

    assert!(matches!(
        artifact
            .validate()
            .expect_err("GO verdict requires objective backtest evidence"),
        ResearchAnalyticsArtifactError::PromotionConfigRequiresGo
    ));
}

#[test]
fn experiment_result_rejects_forbidden_promotion_actions() {
    let mut artifact = valid_experiment_result(RaVerdictKind::Go);
    artifact.promotion_config = Some(promotion_config());
    artifact.accepts_source_proofs = true;
    artifact.mutates_source_proofs = true;
    artifact.mutates_backtest_result_contracts = true;
    artifact.weakens_forbidden_claims = true;
    artifact.notebook_runtime_code_refs = vec![
        "s3://example-bucket/nt-research-analytics/research-analytics/v1/experiment-results/notebook.ipynb"
            .to_string(),
    ];
    artifact.post_verdict_actions = vec![
        ForbiddenPromotionAction::AutoMerge,
        ForbiddenPromotionAction::AutoEnableStrategy,
        ForbiddenPromotionAction::ScheduleLiveTrading,
        ForbiddenPromotionAction::TouchSsmCredentials,
        ForbiddenPromotionAction::MutateProductionRuntimeConfig,
    ];

    let err = artifact
        .validate()
        .expect_err("forbidden post-verdict actions must fail");

    assert!(matches!(
        err,
        ResearchAnalyticsArtifactError::ForbiddenPromotionBehavior { .. }
    ));
    let message = err.to_string();
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
    assert!(message.contains("notebook runtime code"), "{message}");
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
fn experiment_result_requires_source_refs_and_hashes_to_match() {
    let mut artifact = valid_experiment_result(RaVerdictKind::NoGo);
    artifact
        .source_hashes
        .push("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string());

    assert!(matches!(
        artifact
            .validate()
            .expect_err("source refs and hashes must stay one-to-one"),
        ResearchAnalyticsArtifactError::SourceRefHashCountMismatch {
            source_refs: 1,
            source_hashes: 2,
        }
    ));
}

#[test]
fn experiment_result_rejects_stale_content_hash() {
    let mut artifact = valid_experiment_result(RaVerdictKind::NoGo);
    artifact.owner = "research-analytics-drifted".to_string();

    assert!(matches!(
        artifact
            .validate()
            .expect_err("stale experiment-result content hash must fail closed"),
        ResearchAnalyticsArtifactError::ContentHashMismatch { .. }
    ));
}

#[test]
fn experiment_result_rejects_unknown_schema_fields() {
    let artifact = valid_experiment_result(RaVerdictKind::NoGo);
    let mut value = serde_json::to_value(&artifact).expect("serialize experiment result");
    value
        .as_object_mut()
        .expect("experiment result object")
        .insert("future_schema_field".to_string(), serde_json::json!(true));

    assert!(
        serde_json::from_value::<ExperimentResultArtifact>(value).is_err(),
        "unknown experiment-result fields must fail closed"
    );
}

#[test]
fn experiment_result_rejects_cross_family_fidelity_claims() {
    let mut artifact = valid_experiment_result(RaVerdictKind::ConditionalGo);
    let mut snapshot_ref = source_ref(true);
    snapshot_ref.fidelity_class = SourceProofFidelityClass::SnapshotReplay;
    artifact.verdict.source_proof_refs = vec![snapshot_ref];
    artifact.verdict.requested_claim_fidelity = SourceProofFidelityClass::TradeReplay;

    assert!(matches!(
        artifact
            .validate()
            .expect_err("snapshot replay evidence must not imply trade replay claims"),
        ResearchAnalyticsArtifactError::IncompatibleClaimFidelity { .. }
    ));
}

#[test]
fn experiment_result_preserves_dashboard_field_refs_as_read_only_metadata() {
    let mut artifact = valid_experiment_result(RaVerdictKind::ConditionalGo);
    artifact.dashboard_field_refs = vec![
        "dashboard:strategy-candidate-summary:v1".to_string(),
        "dashboard:backtest-evidence-link:v1".to_string(),
    ];
    artifact.content_hash = artifact.expected_content_hash();

    artifact
        .validate()
        .expect("dashboard refs are metadata, not upstream mutations");
}
